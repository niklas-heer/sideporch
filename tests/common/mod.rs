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
    data: TempDir,
}

pub async fn start() -> Server {
    start_with(|_| {}).await
}

/// Starts a server after `configure` adjusts its settings.
pub async fn start_with(configure: impl FnOnce(&mut Config)) -> Server {
    let data = tempfile::tempdir().unwrap();
    let mut config = Config {
        data_dir: data.path().to_owned(),
        public_url: None,
        require_setup_link: false,
        gif_api_base: None,
        allow_insecure_push: true,
        allow_private_link_previews: false,
    };
    configure(&mut config);
    let app = Sideporch::open(config).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let setup_path = app.setup_path();
    let router = app.router();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    Server {
        base,
        setup_path,
        data,
    }
}

impl Server {
    /// The data directory, to check what landed on disk.
    pub fn data_dir(&self) -> &std::path::Path {
        self.data.path()
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

    /// Types a message the way app.js sends it and returns the response,
    /// which is JSON with private notices when it ran a slash command.
    pub async fn type_message(&self, channel_id: i64, body: &str) -> reqwest::Response {
        self.client
            .post(self.url(&format!("/c/{channel_id}/messages")))
            .header("cookie", &self.cookie)
            .header("x-sideporch-fetch", "1")
            .form(&[("body", body)])
            .send()
            .await
            .unwrap()
    }

    /// Posts a message with files, the way app.js does.
    pub async fn upload(
        &self,
        channel_id: i64,
        body: &str,
        files: &[(&str, &[u8])],
    ) -> reqwest::Response {
        let mut form = reqwest::multipart::Form::new().text("body", body.to_owned());
        for (name, data) in files {
            form = form.part(
                "files",
                reqwest::multipart::Part::bytes(data.to_vec()).file_name((*name).to_owned()),
            );
        }
        self.client
            .post(self.url(&format!("/c/{channel_id}/messages")))
            .header("cookie", &self.cookie)
            .header("x-sideporch-fetch", "1")
            .multipart(form)
            .send()
            .await
            .unwrap()
    }

    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> reqwest::Response {
        self.client
            .post(self.url(path))
            .header("cookie", &self.cookie)
            .json(body)
            .send()
            .await
            .unwrap()
    }

    /// Waits until `path` contains `needle`, polling for up to five seconds.
    pub async fn wait_for(&self, path: &str, needle: &str) -> String {
        for _ in 0..50 {
            let page = self.page(path).await;
            if page.contains(needle) {
                return page;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("{needle:?} never appeared on {path}");
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

/// The id of the newest message in a channel page.
pub fn last_message_id(page: &str) -> i64 {
    let list = between(page, r#"id="messages""#, "</ol>");
    list.rsplit(r#"data-message-id=""#)
        .next()
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// A valid 1×1 PNG.
pub const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0,
    0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

pub struct Live {
    socket: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
}

impl Live {
    pub async fn send(&mut self, value: &serde_json::Value) {
        use futures_util::SinkExt;
        self.socket
            .send(Message::Text(value.to_string().into()))
            .await
            .unwrap();
    }

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
