// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

mod common;

use std::time::Duration;

use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
use base64ct::{Base64UrlUnpadded, Encoding as _};
use common::{Browser, Server, admin, home_channel, invite, location, start};
use jiff::{Timestamp, tz::TimeZone};
use reqwest::StatusCode;
use serde_json::json;
use tokio::sync::mpsc;
use web_push_native::p256::{SecretKey, elliptic_curve::sec1::ToEncodedPoint};

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A stand-in push service that records what Sideporch sends it.
async fn push_service() -> (String, mpsc::UnboundedReceiver<(HeaderMap, Bytes)>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let app = Router::new().route(
        "/push/{id}",
        post(move |headers: HeaderMap, body: Bytes| {
            let sender = sender.clone();
            async move {
                sender.send((headers, body)).unwrap();
                StatusCode::CREATED
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, receiver)
}

/// The admin, and Bea with her device subscribed to push, in a direct
/// conversation with the admin.
struct Setup {
    /// Kept so the server runs for the whole test.
    _server: Server,
    admin: Browser,
    bea: Browser,
    dm: i64,
    pushes: mpsc::UnboundedReceiver<(HeaderMap, Bytes)>,
}

async fn setup() -> Setup {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let (service, pushes) = push_service().await;
    let secret = SecretKey::from_slice(&[7; 32]).unwrap();
    let public = secret.public_key().to_encoded_point(false);
    let subscription = json!({
        "endpoint": format!("{service}/push/bea"),
        "keys": {
            "p256dh": Base64UrlUnpadded::encode_string(public.as_bytes()),
            "auth": Base64UrlUnpadded::encode_string(&[9_u8; 16]),
        }
    });
    assert_eq!(
        bea.post_json("/push/subscriptions", &subscription)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let bea_id = bea.user_id().await;
    let dm = location(&admin.get(&format!("/dm/{bea_id}")).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    Setup {
        _server: server,
        admin,
        bea,
        dm,
        pushes,
    }
}

/// Whether a push arrives within `wait`.
async fn pushed(pushes: &mut mpsc::UnboundedReceiver<(HeaderMap, Bytes)>, wait: Duration) -> bool {
    tokio::time::timeout(wait, pushes.recv()).await.is_ok()
}

async fn pause(bea: &Browser, duration: &str) -> StatusCode {
    bea.post("/settings/notifications/pause", &[("duration", duration)])
        .await
        .status()
}

/// Saves quiet hours from the form; an empty `days` list is an error when on.
async fn save_quiet(
    bea: &Browser,
    enabled: bool,
    start: &str,
    end: &str,
    days: &[&str],
) -> reqwest::Response {
    let mut form = vec![("start", start), ("end", end)];
    if enabled {
        form.push(("enabled", "on"));
    }
    form.extend(days.iter().map(|day| ("days", *day)));
    bea.post("/settings/notifications/quiet-hours", &form).await
}

/// `HH:MM` for `offset` minutes from `minute`, wrapped to the day.
fn at(minute: i32, offset: i32) -> String {
    let minute = minute.saturating_add(offset).rem_euclid(1440);
    format!("{:02}:{:02}", minute.div_euclid(60), minute.rem_euclid(60))
}

#[tokio::test]
async fn a_pause_stops_push_until_it_is_resumed() {
    let mut setup = setup().await;
    assert!(
        setup
            .bea
            .page("/settings/notifications")
            .await
            .contains("Notifications are on.")
    );

    assert_eq!(pause(&setup.bea, "1h").await, StatusCode::SEE_OTHER);
    let page = setup.bea.page("/settings/notifications").await;
    assert!(page.contains("Paused until "), "{page}");
    // The account menu shows the same state.
    assert!(setup.bea.page("/activity").await.contains("Paused until "));

    setup.admin.send(setup.dm, "while paused", None).await;
    assert!(!pushed(&mut setup.pushes, Duration::from_millis(800)).await);

    assert_eq!(pause(&setup.bea, "resume").await, StatusCode::SEE_OTHER);
    assert!(
        !setup
            .bea
            .page("/settings/notifications")
            .await
            .contains("Paused until")
    );
    setup.admin.send(setup.dm, "after resuming", None).await;
    assert!(pushed(&mut setup.pushes, Duration::from_secs(5)).await);
}

#[tokio::test]
async fn quiet_hours_follow_the_persons_own_clock() {
    let mut setup = setup().await;
    // Bea's browser reports Berlin time, an hour ahead of UTC in November.
    let mut live = setup.bea.live().await;
    live.send(&json!({ "type": "timezone", "name": "Europe/Berlin" }))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let now = Timestamp::now().to_zoned(TimeZone::get("Europe/Berlin").unwrap());
    let minute = i32::from(now.hour())
        .saturating_mul(60)
        .saturating_add(i32::from(now.minute()));
    let today = DAYS[usize::try_from(now.weekday().to_monday_zero_offset()).unwrap()];
    let everyday: Vec<&str> = DAYS.to_vec();

    // Inside a window of an hour either side of now, on every day.
    let saved = save_quiet(
        &setup.bea,
        true,
        &at(minute, -30),
        &at(minute, 30),
        &everyday,
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    setup.admin.send(setup.dm, "inside the window", None).await;
    assert!(!pushed(&mut setup.pushes, Duration::from_millis(800)).await);

    // Outside it: one hour after now until two hours after.
    save_quiet(
        &setup.bea,
        true,
        &at(minute, 60),
        &at(minute, 120),
        &everyday,
    )
    .await;
    setup.admin.send(setup.dm, "outside the window", None).await;
    assert!(pushed(&mut setup.pushes, Duration::from_secs(5)).await);

    // Crossing midnight and still covering now: it starts at now and ends a
    // minute before it, so it always includes this minute.
    save_quiet(&setup.bea, true, &at(minute, 0), &at(minute, -1), &everyday).await;
    setup.admin.send(setup.dm, "across midnight", None).await;
    assert!(!pushed(&mut setup.pushes, Duration::from_millis(800)).await);

    // Inside the hours, but today isn't one of the days.
    let other_days: Vec<&str> = DAYS.iter().copied().filter(|day| *day != today).collect();
    save_quiet(
        &setup.bea,
        true,
        &at(minute, -30),
        &at(minute, 30),
        &other_days,
    )
    .await;
    setup.admin.send(setup.dm, "wrong day", None).await;
    assert!(pushed(&mut setup.pushes, Duration::from_secs(5)).await);

    // Turning quiet hours off restores push, and keeps the times.
    save_quiet(
        &setup.bea,
        false,
        &at(minute, -30),
        &at(minute, 30),
        &everyday,
    )
    .await;
    setup.admin.send(setup.dm, "switched off", None).await;
    assert!(pushed(&mut setup.pushes, Duration::from_secs(5)).await);
    let page = setup.bea.page("/settings/notifications").await;
    assert!(
        page.contains(&format!("value=\"{}\"", at(minute, -30))),
        "{page}"
    );
    drop(live);
}

#[tokio::test]
async fn activity_keeps_recording_while_paused() {
    let mut setup = setup().await;
    let general = home_channel(&setup.admin).await;
    assert_eq!(pause(&setup.bea, "8h").await, StatusCode::SEE_OTHER);

    setup
        .admin
        .send(general, "@bea can you water the plants?", None)
        .await;
    assert!(!pushed(&mut setup.pushes, Duration::from_millis(800)).await);
    // The mention still reaches Activity.
    assert!(
        setup
            .bea
            .page("/activity")
            .await
            .contains("can you water the plants?")
    );
}

#[tokio::test]
async fn invalid_schedules_are_refused_and_change_nothing() {
    let setup = setup().await;
    assert_eq!(pause(&setup.bea, "2h").await, StatusCode::BAD_REQUEST);
    assert_eq!(pause(&setup.bea, "").await, StatusCode::BAD_REQUEST);

    let refused: [(&str, &str, &[&str], &str); 5] = [
        ("25:00", "07:00", &["mon"], "Pick a start time"),
        ("22:00", "7:00", &["mon"], "Pick an end time"),
        ("08:00", "08:00", &["mon"], "different start and end"),
        ("22:00", "07:00", &[], "at least one day"),
        ("22:00", "07:00", &["funday"], "day of the week"),
    ];
    for (start, end, days, message) in refused {
        let response = save_quiet(&setup.bea, true, start, end, days).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{start} to {end}"
        );
        let text = response.text().await.unwrap();
        assert!(text.contains(message), "{text}");
    }

    // Nothing was saved: the page still says notifications are on.
    let page = setup.bea.page("/settings/notifications").await;
    assert!(page.contains("Notifications are on."));
    assert!(!page.contains("Paused until"));
}
