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

use argon2::{Argon2, password_hash::PasswordHasher};
use common::{Browser, start_in};
use reqwest::StatusCode;

/// A team that started on Sideporch's second schema keeps its account,
/// messages and search after upgrading straight to the newest one.
#[tokio::test]
async fn an_old_database_upgrades_with_its_data() {
    let data = tempfile::tempdir().unwrap();
    let db = data.path().join("sideporch.db");
    sideporch::create_database_at_schema(&db, 2).unwrap();
    let hash = Argon2::default()
        .hash_password(b"correct horse")
        .unwrap()
        .to_string();
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(&format!(
        "INSERT INTO users (id, username, display_name, password_hash, is_admin, created_at)
             VALUES (1, 'ada', 'Ada Admin', '{hash}', 1, 0);
         INSERT INTO channels (id, kind, name, created_by, created_at) VALUES (1, 'public', 'general', 1, 0);
         INSERT INTO channel_members (channel_id, user_id) VALUES (1, 1);
         INSERT INTO messages (id, channel_id, user_id, body, created_at)
             VALUES (1, 1, 1, 'The tomato soup recipe is in the wiki', 0);
         INSERT INTO messages_fts (rowid, content) VALUES (1, 'The tomato soup recipe is in the wiki');
         INSERT INTO reactions (message_id, user_id, emoji, created_at) VALUES (1, 1, 'tada', 0);"
    ))
    .unwrap();
    drop(conn);

    let server = start_in(data, |_| {}).await;
    let mut ada = Browser::anonymous(&server);
    let response = ada
        .submit(
            "/login",
            &[("username", "ada"), ("password", "correct horse")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let page = ada.page("/c/1").await;
    assert!(page.contains("The tomato soup recipe is in the wiki"));
    assert!(page.contains("tada") || page.contains('🎉'));
    let results = ada.page("/search?q=tomato").await;
    assert!(results.contains("recipe"), "{results}");

    let copies: Vec<_> = std::fs::read_dir(server.data_dir().join("upgrade-backups"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(copies.len(), 1);
    assert!(copies[0].starts_with("sideporch-schema2-"), "{copies:?}");
}
