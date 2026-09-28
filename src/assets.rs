//! Static files compiled into the binary.

use axum::{
    Router,
    http::header,
    response::{IntoResponse, Response},
    routing::get,
};

use crate::AppState;

const APP_CSS: &str = include_str!(concat!(env!("OUT_DIR"), "/app.css"));
const APP_JS: &str = include_str!("../assets/app.js");
const EDITOR_JS: &str = include_str!("../assets/editor.js");
/// Mermaid, gzip-compressed; see `assets/vendor/README.md`.
const MERMAID_JS_GZ: &[u8] = include_bytes!("../assets/vendor/mermaid-12.0.0.min.js.gz");
const SERVICE_WORKER: &str = include_str!("../assets/sw.js");
const LOGO: &str = include_str!("../assets/logo.svg");
const MANIFEST: &str = r##"{
  "name": "Sideporch",
  "short_name": "Sideporch",
  "start_url": "/",
  "display": "standalone",
  "background_color": "#24403C",
  "theme_color": "#24403C",
  "icons": [{ "src": "/assets/logo.svg", "sizes": "any", "type": "image/svg+xml" }]
}"##;

/// Versioned URLs (`?v=`) change with their content, so they never expire.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const DAY: &str = "public, max-age=86400";
/// Browsers check the service worker for updates; keep it fresh.
const NO_CACHE: &str = "no-cache";

fn serve(content_type: &'static str, cache: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, cache),
        ],
        body,
    )
        .into_response()
}

macro_rules! font {
    ($file:literal) => {
        get(|| async {
            serve(
                "font/woff2",
                IMMUTABLE,
                include_bytes!(concat!("../assets/fonts/", $file)),
            )
        })
    };
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/assets/app.css",
            get(|| async { serve("text/css; charset=utf-8", IMMUTABLE, APP_CSS.as_bytes()) }),
        )
        .route(
            "/assets/app.js",
            get(|| async {
                serve(
                    "text/javascript; charset=utf-8",
                    IMMUTABLE,
                    APP_JS.as_bytes(),
                )
            }),
        )
        .route(
            "/assets/editor.js",
            get(|| async {
                serve(
                    "text/javascript; charset=utf-8",
                    IMMUTABLE,
                    EDITOR_JS.as_bytes(),
                )
            }),
        )
        .route(
            "/assets/emoji.json",
            get(|| async {
                serve(
                    "application/json",
                    IMMUTABLE,
                    crate::emoji::CATALOG.as_bytes(),
                )
            }),
        )
        .route(
            "/assets/mermaid.js",
            get(|| async {
                (
                    [
                        (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
                        (header::CACHE_CONTROL, IMMUTABLE),
                        (header::CONTENT_ENCODING, "gzip"),
                    ],
                    MERMAID_JS_GZ,
                )
                    .into_response()
            }),
        )
        .route(
            "/sw.js",
            get(|| async {
                serve(
                    "text/javascript; charset=utf-8",
                    NO_CACHE,
                    SERVICE_WORKER.as_bytes(),
                )
            }),
        )
        .route(
            "/assets/logo.svg",
            get(|| async { serve("image/svg+xml", DAY, LOGO.as_bytes()) }),
        )
        .route(
            "/favicon.ico",
            get(|| async { serve("image/svg+xml", DAY, LOGO.as_bytes()) }),
        )
        .route(
            "/manifest.webmanifest",
            get(|| async { serve("application/manifest+json", DAY, MANIFEST.as_bytes()) }),
        )
        .route(
            "/assets/fonts/normal-latin.woff2",
            font!("normal-latin.woff2"),
        )
        .route(
            "/assets/fonts/normal-latin-ext.woff2",
            font!("normal-latin-ext.woff2"),
        )
        .route(
            "/assets/fonts/italic-latin.woff2",
            font!("italic-latin.woff2"),
        )
        .route(
            "/assets/fonts/italic-latin-ext.woff2",
            font!("italic-latin-ext.woff2"),
        )
}
