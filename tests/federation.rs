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

use reqwest::StatusCode;

use common::{Browser, Server, admin, between, start_federated};

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
