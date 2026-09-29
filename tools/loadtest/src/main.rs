//! Simulates people using a Sideporch server.
//!
//! `setup` creates an admin, the accounts and some channels on a fresh
//! server and saves the sessions. `run` then has a number of those people
//! keep a live connection open while looking at one channel each, spread
//! evenly, post messages there at random intervals, and reload it now and
//! then. It reports how long posting takes, how quickly everyone looking at
//! the channel sees each message, and how long pages take.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use clap::{Args, Parser};
use futures_util::{SinkExt, StreamExt, stream};
use hdrhistogram::Histogram;
use rand::Rng;
use reqwest::{Client, StatusCode, redirect::Policy};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue};

type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Parser)]
enum Cli {
    /// Create accounts and channels on a fresh server.
    Setup(SetupArgs),
    /// Simulate people for a while and report latencies.
    Run(RunArgs),
}

#[derive(Args)]
struct SetupArgs {
    #[arg(long)]
    url: String,
    #[arg(long, default_value_t = 100)]
    users: usize,
    #[arg(long, default_value_t = 10)]
    channels: usize,
    /// Accounts created at once; each costs the server an Argon2 hash.
    #[arg(long, default_value_t = 4)]
    concurrency: usize,
    #[arg(long, default_value = "accounts.json")]
    out: PathBuf,
}

#[derive(Args)]
struct RunArgs {
    #[arg(long)]
    url: String,
    #[arg(long, default_value = "accounts.json")]
    accounts: PathBuf,
    /// How many of the accounts take part.
    #[arg(long)]
    users: usize,
    /// Seconds of measuring, after everyone is connected.
    #[arg(long, default_value_t = 60)]
    duration: u64,
    /// Average seconds between one person's messages.
    #[arg(long, default_value_t = 120.0)]
    message_every: f64,
    /// Average seconds between one person's page loads.
    #[arg(long, default_value_t = 300.0)]
    page_every: f64,
    /// Seconds over which people connect.
    #[arg(long, default_value_t = 10)]
    ramp: u64,
}

#[derive(Serialize, Deserialize)]
struct Accounts {
    channels: Vec<i64>,
    /// Session cookies, `sideporch_session=…`.
    sessions: Vec<String>,
}

fn session_cookie(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("sideporch_session="))
        .and_then(|value| value.split(';').next())
        .map(ToOwned::to_owned)
}

fn location(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get("location")?
        .to_str()
        .ok()
        .map(ToOwned::to_owned)
}

async fn setup(args: SetupArgs) -> Result<(), Error> {
    let client = Client::builder().redirect(Policy::none()).build()?;
    let url = args.url.trim_end_matches('/').to_owned();
    let admin = [
        ("display_name", "Admin"),
        ("username", "admin"),
        ("password", "load-test-password"),
    ];
    let created = client
        .post(format!("{url}/setup"))
        .form(&admin)
        .send()
        .await?;
    let admin_cookie = session_cookie(&created).ok_or("setup failed; start from a fresh server")?;
    let mut channels = Vec::new();
    for index in 0..args.channels {
        let response = client
            .post(format!("{url}/channels"))
            .header("cookie", &admin_cookie)
            .form(&[("name", format!("load-{index}"))])
            .send()
            .await?;
        let id = location(&response)
            .and_then(|at| at.rsplit('/').next()?.parse().ok())
            .ok_or("could not create a channel")?;
        channels.push(id);
    }
    client
        .post(format!("{url}/invites"))
        .header("cookie", &admin_cookie)
        .send()
        .await?;
    let people = client
        .get(format!("{url}/people"))
        .header("cookie", &admin_cookie)
        .send()
        .await?
        .text()
        .await?;
    let token = people
        .split("/join/")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_hexdigit()).next())
        .ok_or("no invite link")?
        .to_owned();
    let started = Instant::now();
    let sessions: Vec<Result<String, Error>> = stream::iter(0..args.users)
        .map(|index| {
            let client = client.clone();
            let url = url.clone();
            let token = token.clone();
            async move {
                let response = client
                    .post(format!("{url}/join/{token}"))
                    .form(&[
                        ("display_name", format!("Person {index}")),
                        ("username", format!("p{index}")),
                        ("password", "load-test-password".to_owned()),
                    ])
                    .send()
                    .await?;
                session_cookie(&response)
                    .ok_or_else(|| format!("joining as p{index} failed").into())
            }
        })
        .buffer_unordered(args.concurrency)
        .collect()
        .await;
    let sessions = sessions.into_iter().collect::<Result<Vec<_>, _>>()?;
    eprintln!(
        "created {} accounts and {} channels in {:.1}s",
        sessions.len(),
        channels.len(),
        started.elapsed().as_secs_f64()
    );
    tokio::fs::write(
        &args.out,
        serde_json::to_vec(&Accounts { channels, sessions })?,
    )
    .await?;
    Ok(())
}

/// Latencies in microseconds, merged from every simulated person.
struct Stats {
    post: Mutex<Histogram<u64>>,
    delivery: Mutex<Histogram<u64>>,
    page: Mutex<Histogram<u64>>,
    sent: AtomicU64,
    post_errors: AtomicU64,
    page_errors: AtomicU64,
    delivered: AtomicU64,
    /// Short notices about other channels.
    notices: AtomicU64,
    socket_errors: AtomicU64,
    resyncs: AtomicU64,
    /// Messages sent per channel, by index.
    sent_in: Vec<AtomicU64>,
}

fn histogram() -> Mutex<Histogram<u64>> {
    // Up to five minutes, three significant figures.
    Mutex::new(Histogram::new_with_bounds(1, 300_000_000, 3).expect("valid histogram bounds"))
}

fn record(histogram: &Mutex<Histogram<u64>>, micros: u128) {
    if let Ok(mut histogram) = histogram.lock() {
        let _ = histogram.record(
            u64::try_from(micros)
                .unwrap_or(u64::MAX)
                .clamp(1, 300_000_000),
        );
    }
}

/// Seconds until the next event of a Poisson process with this mean.
fn pause(mean: f64) -> Duration {
    let uniform: f64 = rand::rng().random_range(f64::EPSILON..1.0);
    Duration::from_secs_f64(-uniform.ln() * mean)
}

struct Person {
    index: usize,
    cookie: String,
    /// The channel on this person's screen, and where they write.
    channel: i64,
    /// Its index in the channel list.
    slot: usize,
}

/// Listens on the person's live connection and times other people's
/// messages from posting to arrival.
async fn listen(
    url: String,
    person: Arc<Person>,
    origin: Instant,
    stats: Arc<Stats>,
    until: Instant,
) {
    let ws = url.replacen("http", "ws", 1) + "/ws";
    let Ok(mut request) = ws.into_client_request() else {
        return;
    };
    let Ok(cookie) = HeaderValue::from_str(&person.cookie) else {
        return;
    };
    request.headers_mut().insert("cookie", cookie);
    let Ok((mut socket, _)) = tokio_tungstenite::connect_async(request).await else {
        stats.socket_errors.fetch_add(1, Ordering::Relaxed);
        return;
    };
    // Say which channel is on screen, as the page script does.
    let view = json!({ "type": "view", "channel_id": person.channel }).to_string();
    if socket.send(Message::Text(view.into())).await.is_err() {
        stats.socket_errors.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let own = format!("load#u{}#", person.index);
    let here = format!(r#"{{"type":"message","channel_id":{},"#, person.channel);
    loop {
        let next = tokio::time::timeout_at(until.into(), socket.next()).await;
        let message = match next {
            Err(_) => return,
            Ok(Some(Ok(message))) => message,
            Ok(_) => {
                stats.socket_errors.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        let Message::Text(text) = message else {
            continue;
        };
        if text.contains(r#""type":"resync""#) {
            stats.resyncs.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if !text.starts_with(r#"{"type":"message""#) || text.contains(&own) {
            continue;
        }
        // Messages elsewhere only matter as unread notices.
        if !text.starts_with(&here) || !text.contains(r#""html":"#) {
            stats.notices.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let Some(sent) = text
            .split("load#u")
            .nth(1)
            .and_then(|rest| rest.split("#t").nth(1))
            .and_then(|rest| rest.split('#').next())
            .and_then(|micros| micros.parse::<u128>().ok())
        else {
            continue;
        };
        stats.delivered.fetch_add(1, Ordering::Relaxed);
        record(
            &stats.delivery,
            origin.elapsed().as_micros().saturating_sub(sent),
        );
    }
}

/// Posts messages and loads pages at random intervals until `until`.
async fn talk(
    client: Client,
    url: String,
    person: Arc<Person>,
    origin: Instant,
    stats: Arc<Stats>,
    args: Arc<RunArgs>,
    until: Instant,
) {
    let mut next_message = Instant::now() + pause(args.message_every);
    let mut next_page = Instant::now() + pause(args.page_every);
    loop {
        let next = next_message.min(next_page);
        if next >= until {
            return;
        }
        tokio::time::sleep_until(next.into()).await;
        let channel = person.channel;
        if next == next_message {
            next_message = Instant::now() + pause(args.message_every);
            let body = format!(
                "load#u{}#t{}# Just checking in on the porch, how is everyone doing today?",
                person.index,
                origin.elapsed().as_micros()
            );
            let started = Instant::now();
            let response = client
                .post(format!("{url}/c/{channel}/messages"))
                .header("cookie", &person.cookie)
                .header("x-sideporch-fetch", "1")
                .form(&[("body", body)])
                .send()
                .await;
            match response {
                Ok(response) if response.status() == StatusCode::NO_CONTENT => {
                    stats.sent.fetch_add(1, Ordering::Relaxed);
                    if let Some(count) = stats.sent_in.get(person.slot) {
                        count.fetch_add(1, Ordering::Relaxed);
                    }
                    record(&stats.post, started.elapsed().as_micros());
                }
                _ => {
                    stats.post_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        } else {
            next_page = Instant::now() + pause(args.page_every);
            let started = Instant::now();
            let response = client
                .get(format!("{url}/c/{channel}"))
                .header("cookie", &person.cookie)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    if response.bytes().await.is_ok() {
                        record(&stats.page, started.elapsed().as_micros());
                    }
                }
                _ => {
                    stats.page_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }
}

fn millis(histogram: &Mutex<Histogram<u64>>, quantile: f64) -> f64 {
    histogram.lock().map_or(0.0, |histogram| {
        if histogram.is_empty() {
            0.0
        } else {
            histogram.value_at_quantile(quantile) as f64 / 1000.0
        }
    })
}

async fn run(args: RunArgs) -> Result<(), Error> {
    let accounts: Accounts = serde_json::from_slice(&tokio::fs::read(&args.accounts).await?)?;
    if args.users > accounts.sessions.len() {
        return Err(format!("only {} accounts exist", accounts.sessions.len()).into());
    }
    let url = args.url.trim_end_matches('/').to_owned();
    let client = Client::builder()
        .redirect(Policy::none())
        .pool_max_idle_per_host(args.users)
        .build()?;
    let stats = Arc::new(Stats {
        post: histogram(),
        delivery: histogram(),
        page: histogram(),
        sent: AtomicU64::new(0),
        post_errors: AtomicU64::new(0),
        page_errors: AtomicU64::new(0),
        delivered: AtomicU64::new(0),
        notices: AtomicU64::new(0),
        socket_errors: AtomicU64::new(0),
        resyncs: AtomicU64::new(0),
        sent_in: accounts
            .channels
            .iter()
            .map(|_| AtomicU64::new(0))
            .collect(),
    });
    let origin = Instant::now();
    let connected = origin + Duration::from_secs(args.ramp);
    let until = connected + Duration::from_secs(args.duration);
    // Deliveries of the last messages may still be under way at the end.
    let listen_until = until + Duration::from_secs(5);
    let channels = Arc::new(accounts.channels);
    let args = Arc::new(args);
    let users = args.users;
    let mut tasks = Vec::new();
    for (index, cookie) in accounts.sessions.into_iter().take(users).enumerate() {
        let slot = index % channels.len();
        let person = Arc::new(Person {
            index,
            cookie,
            channel: channels[slot],
            slot,
        });
        let start = origin + Duration::from_secs(args.ramp).mul_f64(index as f64 / users as f64);
        tasks.push(tokio::spawn({
            let (url, stats, person) = (url.clone(), Arc::clone(&stats), Arc::clone(&person));
            async move {
                tokio::time::sleep_until(start.into()).await;
                listen(url, person, origin, stats, listen_until).await;
            }
        }));
        let (client, url, stats, args) = (
            client.clone(),
            url.clone(),
            Arc::clone(&stats),
            Arc::clone(&args),
        );
        tasks.push(tokio::spawn(async move {
            // People start talking once everyone is connected.
            tokio::time::sleep_until(connected.into()).await;
            talk(client, url, person, origin, stats, args, until).await;
        }));
    }
    for task in tasks {
        drop(task.await);
    }
    let sent = stats.sent.load(Ordering::Relaxed);
    // Everyone looking at a channel but the writer should see each message.
    let slots = channels.len();
    let expected: u64 = stats
        .sent_in
        .iter()
        .enumerate()
        .map(|(slot, count)| {
            let viewers = (0..users).filter(|index| index % slots == slot).count();
            count.load(Ordering::Relaxed) * u64::try_from(viewers.saturating_sub(1)).unwrap_or(0)
        })
        .sum();
    let delivered = stats.delivered.load(Ordering::Relaxed);
    let report = json!({
        "users": users,
        "duration_s": args.duration,
        "message_every_s": args.message_every,
        "messages": sent,
        "messages_per_s": sent as f64 / args.duration as f64,
        "post_ms": { "p50": millis(&stats.post, 0.5), "p95": millis(&stats.post, 0.95), "p99": millis(&stats.post, 0.99), "max": millis(&stats.post, 1.0) },
        "delivery_ms": { "p50": millis(&stats.delivery, 0.5), "p95": millis(&stats.delivery, 0.95), "p99": millis(&stats.delivery, 0.99), "max": millis(&stats.delivery, 1.0) },
        "page_ms": { "p50": millis(&stats.page, 0.5), "p95": millis(&stats.page, 0.95), "p99": millis(&stats.page, 0.99) },
        "delivered_ratio": if expected == 0 { 1.0 } else { delivered as f64 / expected as f64 },
        "deliveries_per_s": delivered as f64 / args.duration as f64,
        "notices_per_s": stats.notices.load(Ordering::Relaxed) as f64 / args.duration as f64,
        "post_errors": stats.post_errors.load(Ordering::Relaxed),
        "page_errors": stats.page_errors.load(Ordering::Relaxed),
        "socket_errors": stats.socket_errors.load(Ordering::Relaxed),
        "resyncs": stats.resyncs.load(Ordering::Relaxed),
    });
    println!("{report}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    match Cli::parse() {
        Cli::Setup(args) => setup(args).await,
        Cli::Run(args) => run(args).await,
    }
}
