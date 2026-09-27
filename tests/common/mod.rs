//! Runs a real Sideporch server on a random port and drives it over HTTP,
//! the way a browser or webhook sender would.

#![allow(dead_code)]

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, StatusCode, redirect::Policy};
use sideporch::{Config, Sideporch};
use tempfile::TempDir;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, http::HeaderValue};

pub struct Server {
    pub base: String,
    setup_path: Option<String>,
    _data: TempDir,
}

pub async fn start() -> Server {
    let data = tempfile::tempdir().unwrap();
    let app = Sideporch::open(Config {
        data_dir: data.path().to_owned(),
        public_url: None,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let setup_path = app.setup_path();
    let router = app.router();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server {
        base,
        setup_path,
        _data: data,
    }
}

/// A browser-like client: keeps cookies and does not follow redirects, so
/// tests can check where each form sends the user.
pub struct Browser {
    pub client: Client,
    pub base: String,
    pub cookie: String,
}

impl Browser {
    pub fn anonymous(server: &Server) -> Self {
        Self {
            client: Client::builder().redirect(Policy::none()).build().unwrap(),
            base: server.base.clone(),
            cookie: String::new(),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(self.url(path))
            .header("cookie", &self.cookie)
            .send()
            .await
            .unwrap()
    }

    pub async fn page(&self, path: &str) -> String {
        let response = self.get(path).await;
        assert_eq!(response.status(), StatusCode::OK, "GET {path}");
        response.text().await.unwrap()
    }

    pub async fn post(&self, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("cookie", &self.cookie)
            .form(form)
            .send()
            .await
            .unwrap()
    }

    /// Submits a form and keeps any session cookie the server sets.
    pub async fn submit(&mut self, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
        let response = self.post(path, form).await;
        if let Some(cookie) = response.headers().get("set-cookie") {
            let cookie = cookie.to_str().unwrap();
            cookie
                .split(';')
                .next()
                .unwrap()
                .clone_into(&mut self.cookie);
        }
        response
    }

    /// The user id the server embeds in every signed-in page.
    pub async fn user_id(&self) -> i64 {
        let page = self.page("/home").await;
        between(&page, r#"data-me=""#, "\"").parse().unwrap()
    }

    /// Posts a message the way app.js does and returns the response status.
    pub async fn send(&self, channel_id: i64, body: &str, parent_id: Option<i64>) -> StatusCode {
        let parent = parent_id.map(|id| id.to_string());
        let mut form = vec![("body", body)];
        if let Some(parent) = &parent {
            form.push(("parent_id", parent));
        }
        self.client
            .post(self.url(&format!("/c/{channel_id}/messages")))
            .header("cookie", &self.cookie)
            .header("x-sideporch-fetch", "1")
            .form(&form)
            .send()
            .await
            .unwrap()
            .status()
    }

    pub async fn live(&self) -> Live {
        let url = self.base.replace("http://", "ws://") + "/ws";
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("cookie", HeaderValue::from_str(&self.cookie).unwrap());
        let (socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        Live { socket }
    }
}

pub struct Live {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl Live {
    /// The next event, or `None` if nothing arrives within `wait`.
    pub async fn next_event(&mut self, wait: Duration) -> Option<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let message = tokio::time::timeout_at(deadline, self.socket.next())
                .await
                .ok()??
                .unwrap();
            if let Message::Text(text) = message {
                return Some(serde_json::from_str(&text).unwrap());
            }
        }
    }
}

/// Creates the first account through the setup link and returns its browser.
pub async fn admin(server: &Server) -> Browser {
    let mut browser = Browser::anonymous(server);
    let path = server
        .setup_path
        .clone()
        .expect("fresh server has a setup link");
    let response = browser
        .submit(
            &path,
            &[
                ("display_name", "Ada Admin"),
                ("username", "ada"),
                ("password", "correct horse"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    browser
}

pub fn setup_path(server: &Server) -> String {
    server.setup_path.clone().unwrap()
}

/// Has `admin` create an invite link and uses it to create an account.
pub async fn invite(
    server: &Server,
    admin: &Browser,
    display_name: &str,
    username: &str,
) -> Browser {
    assert_eq!(
        admin.post("/invites", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    let people = admin.page("/people").await;
    let token = between(&people, "/join/", "<");
    let mut browser = Browser::anonymous(server);
    let response = browser
        .submit(
            &format!("/join/{token}"),
            &[
                ("display_name", display_name),
                ("username", username),
                ("password", "a long password"),
            ],
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "joining as {username}"
    );
    browser
}

/// The id of the channel the browser lands on after signing in.
pub async fn home_channel(browser: &Browser) -> i64 {
    let response = browser.get("/").await;
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    location.trim_start_matches("/c/").parse().unwrap()
}

pub fn location(response: &reqwest::Response) -> String {
    response.headers()["location"].to_str().unwrap().to_owned()
}

/// The text between the first `start` and the next `end` after it.
pub fn between<'a>(haystack: &'a str, start: &str, end: &str) -> &'a str {
    let from = haystack
        .find(start)
        .unwrap_or_else(|| panic!("{start:?} not found"))
        + start.len();
    let rest = &haystack[from..];
    &rest[..rest.find(end).unwrap()]
}
