// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Group direct messages: several people in one conversation, driven
//! through a real server.

mod common;

use std::time::Duration;

use common::{Browser, admin, between, invite, location, start};
use reqwest::StatusCode;

/// Opens a group conversation with `people` from `browser`, the way the
/// People page submits its ticked boxes.
async fn start_group(browser: &Browser, people: &[i64]) -> reqwest::Response {
    let keys: Vec<String> = people.iter().map(|id| format!("person_{id}")).collect();
    let form: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "on")).collect();
    browser.post("/dm", &form).await
}

fn channel_of(response: &reqwest::Response) -> i64 {
    location(response)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

#[tokio::test]
async fn a_group_is_named_for_its_people_and_found_again_in_any_order() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let dan = invite(&server, &admin, "Dan", "dan").await;
    let admin_id = admin.user_id().await;
    let bea_id = bea.user_id().await;
    let cat_id = cat.user_id().await;
    let dan_id = dan.user_id().await;

    let created = start_group(&admin, &[bea_id, cat_id]).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let group = channel_of(&created);

    // Each person sees the others' names, never their own.
    let sidebar = admin.page("/home").await;
    assert!(sidebar.contains("Bea and Cat"), "{sidebar}");
    let sidebar = bea.page("/home").await;
    assert!(sidebar.contains("Ada Admin and Cat"), "{sidebar}");
    assert!(
        admin
            .page(&format!("/c/{group}"))
            .await
            .contains("Message Bea and Cat")
    );

    // The same people in any order, with the creator named among them,
    // open the same conversation.
    assert_eq!(
        channel_of(&start_group(&cat, &[admin_id, bea_id]).await),
        group
    );
    assert_eq!(
        channel_of(&start_group(&bea, &[cat_id, admin_id, bea_id]).await),
        group
    );
    assert_eq!(
        channel_of(&start_group(&admin, &[bea_id, cat_id, admin_id]).await),
        group
    );
    // A different set is a different conversation.
    let other = channel_of(&start_group(&admin, &[bea_id, cat_id, dan_id]).await);
    assert_ne!(other, group);
    assert_eq!(
        start_group(&admin, &[]).await.status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn every_member_is_told_and_sees_the_conversation_unread() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let bea_id = bea.user_id().await;
    let cat_id = cat.user_id().await;
    let group = channel_of(&start_group(&admin, &[bea_id, cat_id]).await);

    let mut bea_live = bea.live().await;
    let mut cat_live = cat.live().await;
    assert_eq!(
        admin.send(group, "Lunch at noon?", None).await,
        StatusCode::NO_CONTENT
    );
    for live in [&mut bea_live, &mut cat_live] {
        let event = live
            .next_event(Duration::from_secs(5))
            .await
            .expect("a notice for the group");
        assert_eq!(event["channel_id"], group);
    }

    for member in [&bea, &cat] {
        let sidebar = member.page("/home").await;
        let link = between(&sidebar, &format!(r#"data-channel-link="{group}""#), ">");
        assert!(link.contains("data-unread"), "{link}");
        assert!(
            member
                .page(&format!("/c/{group}"))
                .await
                .contains("Lunch at noon?")
        );
        let sidebar = member.page("/home").await;
        let link = between(&sidebar, &format!(r#"data-channel-link="{group}""#), ">");
        assert!(!link.contains("data-unread"), "reading clears the marker");
    }
}

#[tokio::test]
async fn a_group_is_private_to_its_members_in_channels_messages_and_search() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let dan = invite(&server, &admin, "Dan", "dan").await;
    let bea_id = bea.user_id().await;
    let cat_id = cat.user_id().await;
    let group = channel_of(&start_group(&admin, &[bea_id, cat_id]).await);
    assert_eq!(
        admin.send(group, "Quietly, the zebra plan", None).await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        dan.get(&format!("/c/{group}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        dan.send(group, "let me in", None).await,
        StatusCode::NOT_FOUND
    );
    let results = dan.page("/search?q=zebra").await;
    assert!(!results.contains("zebra plan"), "{results}");
    assert!(
        !dan.page("/home")
            .await
            .contains(&format!(r#"data-channel-link="{group}""#))
    );

    // The members still find it in search, labelled with the others' names.
    let results = bea.page("/search?q=zebra").await;
    assert!(results.contains("Quietly, the <mark"), "{results}");
    assert!(results.contains("Ada Admin and Cat"), "{results}");
}

#[tokio::test]
async fn a_group_takes_two_to_nine_people_and_refuses_people_it_cannot_hold() {
    let server = start().await;
    let admin = admin(&server).await;
    let mut ids = Vec::new();
    for n in 0..9 {
        let person = invite(
            &server,
            &admin,
            &format!("Member {n}"),
            &format!("member{n}"),
        )
        .await;
        ids.push(person.user_id().await);
    }

    // Nine people in all, the creator included, is the most.
    let full = start_group(&admin, &ids[..8]).await;
    assert_eq!(full.status(), StatusCode::SEE_OTHER);
    let too_many = start_group(&admin, &ids).await;
    assert_eq!(too_many.status(), StatusCode::BAD_REQUEST);
    let message = too_many.text().await.unwrap();
    assert!(message.contains("up to 9 people"), "{message}");

    // Someone from another server can't be added.
    let elsewhere = invite(&server, &admin, "Elsewhere", "elsewhere").await;
    let elsewhere_id = elsewhere.user_id().await;
    let conn = rusqlite::Connection::open(server.data_dir().join("sideporch.db")).unwrap();
    conn.execute(
        "INSERT INTO instances (url, handle, status, created_at, updated_at)
         VALUES ('https://far.example', 'far.example', 'connected', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE users SET instance_id = last_insert_rowid(), remote_id = 7 WHERE id = ?1",
        [elsewhere_id],
    )
    .unwrap();
    drop(conn);
    let refused = start_group(&admin, &[ids[0], elsewhere_id]).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(refused.text().await.unwrap().contains("other servers"));

    // Nor can a deactivated account.
    let gone = invite(&server, &admin, "Gone", "gone").await;
    let gone_id = gone.user_id().await;
    assert_eq!(
        admin
            .post(&format!("/people/{gone_id}/deactivate"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let refused = start_group(&admin, &[ids[0], gone_id]).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(refused.text().await.unwrap().contains("Deactivated"));
}

#[tokio::test]
async fn two_people_keep_their_direct_message_and_their_notes_to_self() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let bea_id = bea.user_id().await;
    let admin_id = admin.user_id().await;

    let direct = location(&admin.get(&format!("/dm/{bea_id}")).await);
    let direct_id: i64 = direct.trim_start_matches("/c/").parse().unwrap();
    assert_eq!(
        admin.send(direct_id, "Still just us", None).await,
        StatusCode::NO_CONTENT
    );
    // A group of two is the same conversation as the direct message.
    assert_eq!(channel_of(&start_group(&bea, &[admin_id]).await), direct_id);
    assert!(bea.page(&direct).await.contains("Still just us"));
    assert!(bea.page("/home").await.contains("Ada Admin"));

    // Notes to self stay on their own route.
    assert_eq!(
        admin.get(&format!("/dm/{admin_id}")).await.status(),
        StatusCode::SEE_OTHER
    );
}
