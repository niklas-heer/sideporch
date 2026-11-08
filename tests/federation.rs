//! Two Sideporch servers connecting and sharing channels and conversations,
//! each a real server on this machine.

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

use base64ct::Encoding as _;
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use sha2::Digest as _;

use std::time::Duration;

use common::{Browser, Live, Server, admin, between, start_federated};

/// The part of a server's address people's names carry: `127.0.0.1:1234`.
fn handle(server: &Server) -> String {
    server.base.trim_start_matches("http://").to_owned()
}

/// The path of the first `action` button on a connections page, such as
/// `/admin/connections/3/accept`.
fn action_path(page: &str, action: &str) -> String {
    let end = page.find(&format!("/{action}\"")).expect(action) + action.len() + 1;
    let start = page[..end].rfind("/admin/connections/").unwrap();
    page[start..end].to_owned()
}

/// Asks `to` to connect from `from`, and has `to`'s admin accept.
async fn connect(from: &Browser, to: &Browser, to_server: &Server) {
    let response = from
        .post(
            "/admin/connections",
            &[
                ("url", to_server.base.as_str()),
                ("note", "Hello from the garden club"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let page = to.wait_for("/admin/connections", "wants to connect").await;
    assert_eq!(
        to.post(&action_path(&page, "accept"), &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    from.wait_for("/admin/connections", r#"data-status="connected""#)
        .await;
}

#[tokio::test]
async fn two_servers_connect_once_both_admins_agree() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);

    let own = ada.page("/admin/connections").await;
    let a_fingerprint = between(&own, "data-fingerprint>", "<");
    assert_eq!(a_fingerprint.split(' ').count(), 8, "{own}");

    ada.post(
        "/admin/connections",
        &[
            ("url", b.base.as_str()),
            ("note", "Hello from the garden club"),
        ],
    )
    .await;
    let asked = ada.page("/admin/connections").await;
    assert!(asked.contains("Waiting for them to accept"), "{asked}");

    // B's admin sees who asks, their note and their key's fingerprint.
    let request = bo.wait_for("/admin/connections", "wants to connect").await;
    assert!(request.contains(&handle(&a)), "{request}");
    assert!(request.contains("Hello from the garden club"), "{request}");
    assert!(
        request.contains(a_fingerprint),
        "the fingerprint to compare: {request}"
    );

    assert_eq!(
        bo.post(&action_path(&request, "accept"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert!(
        bo.page("/admin/connections")
            .await
            .contains(r#"data-status="connected""#)
    );
    let connected = ada
        .wait_for("/admin/connections", r#"data-status="connected""#)
        .await;
    assert!(connected.contains(&handle(&b)), "{connected}");
}

#[tokio::test]
async fn a_declined_request_connects_nothing() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    ada.post(
        "/admin/connections",
        &[("url", b.base.as_str()), ("note", "")],
    )
    .await;
    let request = bo.wait_for("/admin/connections", "wants to connect").await;
    bo.post(&action_path(&request, "decline"), &[]).await;
    ada.wait_for("/admin/connections", r#"data-status="declined""#)
        .await;
    assert!(
        !bo.page("/admin/connections")
            .await
            .contains("wants to connect")
    );
}

#[tokio::test]
async fn only_admins_manage_connections() {
    let (a, b) = (start_federated().await, start_federated().await);
    let ada = admin(&a).await;
    let cy = common::invite(&a, &ada, "Cy", "cy").await;
    assert_eq!(
        cy.get("/admin/connections").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        cy.post(
            "/admin/connections",
            &[("url", b.base.as_str()), ("note", "")]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn servers_answer_only_signed_requests_from_connected_servers() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;
    let client = reqwest::Client::new();
    // No signature.
    let unsigned = client
        .post(format!("{}/federation/inbox", b.base))
        .body("{\"events\": []}")
        .send()
        .await
        .unwrap();
    assert_eq!(unsigned.status(), StatusCode::UNAUTHORIZED);
    // Claims to be A, but signed with another key.
    let forged = client
        .post(format!("{}/federation/inbox", b.base))
        .header("sideporch-server", &a.base)
        .header("sideporch-date", "0")
        .header("sideporch-nonce", "abc")
        .header("sideporch-signature", "AAAA")
        .body("{\"events\": []}")
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::UNAUTHORIZED);
    // The public description needs no signature.
    let known: serde_json::Value = client
        .get(format!("{}/.well-known/sideporch", b.base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(known["url"], b.base.as_str());
    assert!(
        known["public_key"]
            .as_str()
            .is_some_and(|key| !key.is_empty())
    );
}

/// Creates a channel on `admin`'s server and returns its id.
async fn channel(admin: &Browser, name: &str, private: bool) -> i64 {
    let mut form = vec![("name", name)];
    if private {
        form.push(("private", "on"));
    }
    let response = admin.post("/channels", &form).await;
    common::location(&response)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

/// The id another server has on `admin`'s server.
async fn server_id(admin: &Browser) -> String {
    let page = admin.page("/admin/connections").await;
    let path = action_path(&page, "disconnect");
    path.trim_start_matches("/admin/connections/")
        .trim_end_matches("/disconnect")
        .to_owned()
}

/// Shares `channel` from `host`'s server with the guest's, whose admin
/// accepts. Returns the guest's copy.
async fn share(host: &Browser, guest: &Browser, channel: i64) -> i64 {
    let server = server_id(host).await;
    let response = host
        .post(
            &format!("/c/{channel}/share"),
            &[("server", server.as_str())],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let offers = guest
        .wait_for("/admin/connections", "shared with this server")
        .await;
    let accepted = guest.post(&action_path(&offers, "take"), &[]).await;
    assert_eq!(accepted.status(), StatusCode::SEE_OTHER);
    common::location(&accepted)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

#[tokio::test]
async fn a_shared_channel_carries_conversations_both_ways() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;
    let bea = common::invite(&b, &bo, "Bea", "bea").await;

    let garden = channel(&ada, "garden", false).await;
    ada.send(garden, "Welcome to the garden", None).await;
    let copy = share(&ada, &bo, garden).await;

    // Both channels' settings say who takes part.
    ada.wait_for(
        &format!("/c/{garden}/settings"),
        &format!(r#"data-shared-with="{}""#, handle(&b)),
    )
    .await;
    let settings = bo.page(&format!("/c/{copy}/settings")).await;
    assert!(
        settings.contains(&format!(r#"data-shared-by="{}""#, handle(&a))),
        "{settings}"
    );
    assert!(
        !settings.contains(&format!("/c/{copy}/share\"")),
        "{settings}"
    );

    // What was said before sharing comes along.
    bea.post(&format!("/c/{copy}/join"), &[]).await;
    let page = bea
        .wait_for(&format!("/c/{copy}"), "Welcome to the garden")
        .await;
    assert!(page.contains("Ada Admin"), "{page}");

    // B to A, named by server.
    bea.send(copy, "Hello from the other porch", None).await;
    let page = ada
        .wait_for(&format!("/c/{garden}"), "Hello from the other porch")
        .await;
    assert!(page.contains(&format!("bea@{}", handle(&b))), "{page}");
    let hello = common::last_message_id(&page);

    // A answers in the thread; B sees it in the thread.
    ada.send(
        garden,
        &format!("Welcome @bea@{}!", handle(&b)),
        Some(hello),
    )
    .await;
    let bea_hello = common::last_message_id(&bea.page(&format!("/c/{copy}")).await);
    // On her own server, the mention names her there, highlighted.
    bea.wait_for(
        &format!("/c/{copy}/t/{bea_hello}"),
        r#"Welcome <span class="font-semibold">@bea</span>!"#,
    )
    .await;
    // Mentioned on another server, notified on her own.
    bea.wait_for("/activity", "Welcome").await;

    // Edits, reactions and deletion follow.
    bea.post(
        &format!("/c/{copy}/m/{bea_hello}/edit"),
        &[("body", "Hello from the other porch, edited")],
    )
    .await;
    ada.wait_for(&format!("/c/{garden}"), "the other porch, edited")
        .await;
    ada.post(
        &format!("/c/{garden}/m/{hello}/reactions"),
        &[("emoji", "tada")],
    )
    .await;
    bea.wait_for(&format!("/c/{copy}"), "🎉").await;
    bea.post(&format!("/c/{copy}/m/{bea_hello}/delete"), &[])
        .await;
    for _ in 0..200 {
        if !ada
            .page(&format!("/c/{garden}"))
            .await
            .contains("the other porch")
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("the deleted message stayed on A");
}

/// The conversation with `name` in a page's sidebar.
fn conversation_with(page: &str, name: &str) -> i64 {
    let at = page
        .find(&format!(">{name}<"))
        .unwrap_or_else(|| panic!("no {name}: {page}"));
    let start = page[..at].rfind(r#"href="/c/"#).unwrap() + r#"href="/c/"#.len();
    let end = start + page[start..].find('"').unwrap();
    page[start..end].parse().unwrap()
}

#[tokio::test]
async fn people_write_directly_across_servers() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;
    let bea = common::invite(&b, &bo, "Bea Baker", "bea").await;

    // Ada finds Bea on the connected server.
    let found = ada.page("/people/elsewhere?q=bea").await;
    assert!(found.contains("Bea Baker"), "{found}");
    let form = between(&found, r#"action="/people/elsewhere/message""#, "</form>");
    let field = |name: &str| between(form, &format!(r#"name="{name}" value=""#), "\"").to_owned();
    let (server, id) = (field("server"), field("id"));
    let response = ada
        .post(
            "/people/elsewhere/message",
            &[("server", server.as_str()), ("id", id.as_str())],
        )
        .await;
    let conversation: i64 = common::location(&response)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    ada.send(conversation, "Hi Bea, it's Ada from next door", None)
        .await;

    // Bea has a direct conversation with Ada now, and answers.
    let page = bea.wait_for("/people", ">Ada Admin<").await;
    let hers = conversation_with(&page, "Ada Admin");
    bea.wait_for(&format!("/c/{hers}"), "Ada from next door")
        .await;
    bea.send(hers, "Hi Ada!", None).await;
    ada.wait_for(&format!("/c/{conversation}"), "Hi Ada!").await;
}

#[tokio::test]
async fn a_private_shared_channel_is_a_group_of_people_from_both_servers() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;
    let bea = common::invite(&b, &bo, "Bea", "bea").await;
    let cy = common::invite(&b, &bo, "Cy", "cy").await;

    let plans = channel(&ada, "offsite-plans", true).await;
    let copy = share(&ada, &bo, plans).await;
    // Each server adds its own people.
    let bea_id = bea.user_id().await.to_string();
    bo.post(
        &format!("/c/{copy}/members"),
        &[("user_id", bea_id.as_str())],
    )
    .await;
    bea.send(copy, "Count me in for the offsite", None).await;
    ada.wait_for(&format!("/c/{plans}"), "Count me in").await;
    ada.send(plans, "Great, see you there", None).await;
    bea.wait_for(&format!("/c/{copy}"), "see you there").await;
    // Not a member, not in the group.
    assert_ne!(cy.get(&format!("/c/{copy}")).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn disconnecting_ends_sharing_but_keeps_what_was_said() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;
    let garden = channel(&ada, "garden", false).await;
    let copy = share(&ada, &bo, garden).await;
    bo.post(&format!("/c/{copy}/join"), &[]).await;
    bo.send(copy, "Before the goodbye", None).await;
    ada.wait_for(&format!("/c/{garden}"), "Before the goodbye")
        .await;

    let server = server_id(&ada).await;
    ada.post(&format!("/admin/connections/{server}/disconnect"), &[])
        .await;
    bo.wait_for("/admin/connections", r#"data-status="disconnected""#)
        .await;
    bo.send(copy, "Anyone there?", None).await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let page = ada.page(&format!("/c/{garden}")).await;
    assert!(page.contains("Before the goodbye"), "{page}");
    assert!(!page.contains("Anyone there?"), "{page}");
}

#[tokio::test]
async fn nobody_signs_in_as_someone_from_another_server() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;
    let garden = channel(&ada, "garden", false).await;
    let copy = share(&ada, &bo, garden).await;
    ada.send(garden, "Hello from A", None).await;
    let page = bo.wait_for(&format!("/c/{copy}"), "Hello from A").await;
    // Ada's account on B, from her message there.
    let at = page.find("Hello from A").unwrap();
    let ada_on_b = {
        let start = page[..at].rfind(r#"href="/people/"#).unwrap() + r#"href="/people/"#.len();
        page[start..start + page[start..].find('"').unwrap()].to_owned()
    };
    // B's admin can't hand out a way in, and People lists only B's own.
    let refused = bo
        .post(&format!("/people/{ada_on_b}/reset-link"), &[])
        .await;
    assert_ne!(refused.status(), StatusCode::OK);
    let people = bo.page("/people").await;
    assert!(
        !people.contains(&format!("@ada@{}", handle(&a))),
        "{people}"
    );
    assert!(people.contains(r#"href="/people/elsewhere""#), "{people}");
    // Her profile there says where she's from, with nothing to administer.
    let profile = bo.page(&format!("/people/{ada_on_b}")).await;
    assert!(
        profile.contains(&format!(r#"data-server="{}""#, handle(&a))),
        "{profile}"
    );
    assert!(!profile.contains("reset-link"), "{profile}");
    let mut stranger = Browser::anonymous(&b);
    let login = stranger
        .submit(
            "/login",
            &[
                ("username", &format!("ada@{}", handle(&a))),
                ("password", ""),
            ],
        )
        .await;
    assert_ne!(login.status(), StatusCode::SEE_OTHER);
}

// Typing notices: best effort, signed like other requests, never stored.

/// The next typing event on `live`, or none within `wait`.
async fn typing_within(live: &mut Live, wait: Duration) -> Option<serde_json::Value> {
    let deadline = tokio::time::Instant::now() + wait;
    while let Some(event) = live
        .next_event(deadline.saturating_duration_since(tokio::time::Instant::now()))
        .await
    {
        if event["type"] == "typing" {
            return Some(event);
        }
    }
    None
}

/// The database file a server keeps, opened to seed or inspect it.
fn database(server: &Server) -> rusqlite::Connection {
    rusqlite::Connection::open(server.data_dir().join("sideporch.db")).unwrap()
}

/// Makes `server` know the one at `url` as connected, as if they had
/// connected. Returns the id it has for them.
fn connected_peer(server: &Server, handle: &str, url: &str, key: Option<&[u8]>) -> i64 {
    let conn = database(server);
    conn.execute(
        "INSERT INTO instances (url, handle, name, public_key, status, note, allow_direct, created_at, updated_at)
         VALUES (?1, ?2, ?2, ?3, 'connected', '', 1, 0, 0)",
        rusqlite::params![url, handle, key],
    )
    .unwrap();
    conn.last_insert_rowid()
}

/// Shares `channel` with `peer`, as `role` (`host` means the peer hosts it).
fn share_with_peer(server: &Server, channel: i64, peer: i64, role: &str, remote: Option<i64>) {
    database(server)
        .execute(
            "INSERT INTO shared_channels (channel_id, instance_id, role, remote_channel_id, status, created_at)
             VALUES (?1, ?2, ?3, ?4, 'active', 0)",
            rusqlite::params![channel, peer, role, remote],
        )
        .unwrap();
}

/// How many rows a server's outbox holds.
fn outbox_len(server: &Server) -> i64 {
    database(server)
        .query_row("SELECT COUNT(*) FROM federation_outbox", [], |row| {
            row.get(0)
        })
        .unwrap()
}

/// Waits until a server's outbox is empty, so later counts are its own.
async fn outbox_drained(server: &Server) {
    for _ in 0..100 {
        if outbox_len(server) == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the outbox never emptied");
}

/// How many accounts stand in for people from other servers on `server`.
fn stand_ins(server: &Server) -> i64 {
    database(server)
        .query_row(
            "SELECT COUNT(*) FROM users WHERE instance_id IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

/// A typing notice body for `person` (a name on `server`) in `channel`.
fn notice(channel: i64, parent: Option<&str>, server: &str, name: &str) -> Vec<u8> {
    serde_json::json!({
        "channel": channel,
        "parent": parent,
        "person": {
            "server": server,
            "url": format!("https://{server}"),
            "id": 7,
            "username": "rob",
            "display_name": name,
        },
        "stopped": false,
    })
    .to_string()
    .into_bytes()
}

/// The headers a server sends with a signed `POST` of `body` to `path` on
/// `target`, as `sender`, with `signer`'s key.
fn sign(
    signer: &ring::signature::Ed25519KeyPair,
    sender: &str,
    target: &str,
    path: &str,
    body: &[u8],
    nonce: &str,
) -> Vec<(&'static str, String)> {
    let date = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis()
        .to_string();
    let hash = sha2::Sha256::digest(body)
        .iter()
        .fold(String::new(), |mut hex, byte| {
            use std::fmt::Write as _;
            // Writing to a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        });
    let input =
        format!("sideporch-request-v1\nPOST\n{path}\n{target}\n{sender}\n{date}\n{nonce}\n{hash}");
    vec![
        ("sideporch-server", sender.to_owned()),
        ("sideporch-date", date),
        ("sideporch-nonce", nonce.to_owned()),
        (
            "sideporch-signature",
            base64ct::Base64::encode_string(signer.sign(input.as_bytes()).as_ref()),
        ),
    ]
}

/// A new signing key, as a server that connected to `b` would have.
fn new_key() -> ring::signature::Ed25519KeyPair {
    let pkcs8 =
        ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
    ring::signature::Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
}

/// Posts a typing notice to `server` with `headers`, and returns its answer.
async fn deliver(server: &Server, headers: &[(&str, String)], body: Vec<u8>) -> StatusCode {
    let mut request = reqwest::Client::new()
        .post(format!("{}/federation/typing", server.base))
        .body(body);
    for (name, value) in headers {
        request = request.header(*name, value);
    }
    request.send().await.unwrap().status()
}

#[tokio::test]
async fn typing_crosses_to_the_servers_a_channel_is_shared_with() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;

    let garden = channel(&ada, "garden", false).await;
    ada.send(garden, "Welcome to the garden", None).await;
    let hello = common::last_message_id(&ada.page(&format!("/c/{garden}")).await);
    let copy = share(&ada, &bo, garden).await;
    bo.wait_for(&format!("/c/{copy}"), "Welcome to the garden")
        .await;
    let bo_hello = common::last_message_id(&bo.page(&format!("/c/{copy}")).await);
    outbox_drained(&a).await;
    let stand_ins_before = stand_ins(&b);

    // Someone on B is looking at the copy, and Ada types on A.
    let mut watching = bo.live_in(copy).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut ada_typing = ada.live_in(garden).await;
    ada_typing
        .send(&serde_json::json!({
            "type": "typing", "channel_id": garden, "parent_id": null, "stopped": false
        }))
        .await;
    let seen = typing_within(&mut watching, Duration::from_secs(5))
        .await
        .expect("typing on B");
    assert_eq!(seen["channel_id"], copy);
    assert_eq!(seen["name"], "Ada Admin");
    assert_eq!(seen["parent_id"], serde_json::Value::Null);

    // In a thread: named by its first message, on each side.
    let mut ada_in_thread = ada.live_in(garden).await;
    ada_in_thread
        .send(&serde_json::json!({
            "type": "typing", "channel_id": garden, "parent_id": hello, "stopped": false
        }))
        .await;
    let thread = typing_within(&mut watching, Duration::from_secs(5))
        .await
        .expect("typing in the thread on B");
    assert_eq!(thread["channel_id"], copy);
    assert_eq!(thread["parent_id"], bo_hello);

    // Typing is never queued, and makes no account here.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(outbox_len(&a), 0, "typing went into the outbox");
    assert_eq!(stand_ins(&b), stand_ins_before, "typing made an account");
}

#[tokio::test]
async fn typing_is_shown_only_for_people_the_sender_may_speak_for() {
    let b = start_federated().await;
    let bo = admin(&b).await;
    let mallory_url = "http://mallory.test";
    let key = new_key();
    let peer = connected_peer(
        &b,
        "mallory.test",
        mallory_url,
        Some(key.public_key().as_ref()),
    );
    let target = b.base.as_str();
    let path = "/federation/typing";
    let garden = channel(&bo, "garden", false).await;
    let hosted = channel(&bo, "hosted", false).await;
    let plans = channel(&bo, "plans", true).await;
    let secret = channel(&bo, "secret", true).await;
    // Mallory hosts a copy of `garden` and `plans`; `hosted` is B's own,
    // which Mallory keeps a copy of.
    share_with_peer(&b, garden, peer, "guest", Some(55));
    share_with_peer(&b, plans, peer, "guest", Some(56));
    share_with_peer(&b, hosted, peer, "host", None);
    let mut watch_garden = bo.live_in(garden).await;
    let mut watch_hosted = bo.live_in(hosted).await;
    let mut watch_plans = bo.live_in(plans).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let quiet = Duration::from_millis(500);

    // Mallory's own person, in a channel it hosts: shown.
    let accepted = notice(55, None, "mallory.test", "Rob Rogue");
    let headers = sign(&key, mallory_url, target, path, &accepted, "one");
    assert_eq!(
        deliver(&b, &headers, accepted.clone()).await,
        StatusCode::NO_CONTENT
    );
    let seen = typing_within(&mut watch_garden, Duration::from_secs(5))
        .await
        .expect("typing shown");
    assert_eq!(seen["channel_id"], garden);
    assert_eq!(seen["name"], "Rob Rogue");
    assert_eq!(seen["stopped"], false);
    // Showing it made no account for Rob here.
    let rob: i64 = database(&b)
        .query_row(
            "SELECT COUNT(*) FROM users WHERE username = 'rob@mallory.test'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(rob, 0);

    // Replaying the same request is refused.
    assert_eq!(
        deliver(&b, &headers, accepted).await,
        StatusCode::UNAUTHORIZED
    );

    // Mallory is a guest of `hosted`, so it may speak only for its own
    // people there: not for someone elsewhere, nor for someone on B.
    let elsewhere = notice(hosted, None, "other.test", "Olive Other");
    let headers = sign(&key, mallory_url, target, path, &elsewhere, "two");
    assert_eq!(
        deliver(&b, &headers, elsewhere).await,
        StatusCode::FORBIDDEN
    );
    let here = notice(hosted, None, &handle(&b), "Bo Admin");
    let headers = sign(&key, mallory_url, target, path, &here, "three");
    assert_eq!(deliver(&b, &headers, here).await, StatusCode::FORBIDDEN);
    assert!(typing_within(&mut watch_hosted, quiet).await.is_none());
    // Its own person in `hosted` is shown.
    let own = notice(hosted, None, "mallory.test", "Rob Rogue");
    let headers = sign(&key, mallory_url, target, path, &own, "four");
    assert_eq!(deliver(&b, &headers, own).await, StatusCode::NO_CONTENT);
    assert!(
        typing_within(&mut watch_hosted, Duration::from_secs(5))
            .await
            .is_some()
    );

    // A channel that isn't shared with Mallory, shared or not: refused.
    let unshared = notice(secret, None, "mallory.test", "Rob Rogue");
    let headers = sign(&key, mallory_url, target, path, &unshared, "five");
    assert_eq!(deliver(&b, &headers, unshared).await, StatusCode::FORBIDDEN);

    // A private channel: only its members, and Rob isn't one.
    let private = notice(56, None, "mallory.test", "Rob Rogue");
    let headers = sign(&key, mallory_url, target, path, &private, "six");
    assert_eq!(deliver(&b, &headers, private).await, StatusCode::FORBIDDEN);
    assert!(typing_within(&mut watch_plans, quiet).await.is_none());

    // Signed with another key: refused.
    let forged_key = new_key();
    let forged = notice(55, None, "mallory.test", "Rob Rogue");
    let headers = sign(&forged_key, mallory_url, target, path, &forged, "seven");
    assert_eq!(
        deliver(&b, &headers, forged).await,
        StatusCode::UNAUTHORIZED
    );
    assert!(typing_within(&mut watch_garden, quiet).await.is_none());
}

#[tokio::test]
async fn typing_in_a_private_shared_channel_reaches_its_members_only() {
    let (a, b) = (start_federated().await, start_federated().await);
    let (ada, bo) = (admin(&a).await, admin(&b).await);
    connect(&ada, &bo, &b).await;
    let bea = common::invite(&b, &bo, "Bea", "bea").await;
    let cy = common::invite(&b, &bo, "Cy", "cy").await;

    let plans = channel(&ada, "offsite-plans", true).await;
    let copy = share(&ada, &bo, plans).await;
    let bea_id = bea.user_id().await.to_string();
    bo.post(
        &format!("/c/{copy}/members"),
        &[("user_id", bea_id.as_str())],
    )
    .await;
    let mut bea_live = bea.live_in(copy).await;
    let mut cy_live = cy.live_in(copy).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let mut ada_typing = ada.live_in(plans).await;
    ada_typing
        .send(&serde_json::json!({
            "type": "typing", "channel_id": plans, "parent_id": null, "stopped": false
        }))
        .await;
    let seen = typing_within(&mut bea_live, Duration::from_secs(5))
        .await
        .expect("typing for a member");
    assert_eq!(seen["channel_id"], copy);
    assert_eq!(seen["name"], "Ada Admin");
    // Cy isn't in the channel, so the notice doesn't reach him.
    assert!(
        typing_within(&mut cy_live, Duration::from_millis(500))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn a_slow_or_down_peer_does_not_hold_back_the_sender() {
    let a = start_federated().await;
    let ada = admin(&a).await;
    let garden = channel(&ada, "garden", false).await;
    // One peer accepts connections and never answers.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let silent_address = silent.local_addr().unwrap().to_string();
    let silent_peer = connected_peer(
        &a,
        &silent_address,
        &format!("http://{silent_address}"),
        None,
    );
    share_with_peer(&a, garden, silent_peer, "host", None);
    // Another is down: nothing listens there.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_address = closed.local_addr().unwrap().to_string();
    drop(closed);
    let down_peer = connected_peer(
        &a,
        &closed_address,
        &format!("http://{closed_address}"),
        None,
    );
    share_with_peer(&a, garden, down_peer, "host", None);

    let mut watching = ada.live_in(garden).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = tokio::time::Instant::now();
    watching
        .send(&serde_json::json!({
            "type": "typing", "channel_id": garden, "parent_id": null, "stopped": false
        }))
        .await;
    assert!(
        typing_within(&mut watching, Duration::from_secs(1))
            .await
            .is_some(),
        "the sender's own typing shows at once"
    );
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(outbox_len(&a), 0, "typing went into the outbox");
    drop(silent);
}
