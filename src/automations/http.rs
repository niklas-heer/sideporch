//! Outgoing HTTP for `sideporch.http`.
//!
//! Scripts run on plain threads, so requests block them while the shared
//! Tokio runtime does the work. By default, requests may not reach private,
//! loopback, link-local or other internal addresses, so a script cannot be
//! used to probe the server's network. The check happens inside DNS
//! resolution, so a name cannot resolve to a public address when checked and
//! an internal one when connecting. Admins can allow internal addresses for
//! home-lab setups. Redirects are not followed.

use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::body::Bytes;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{
        Client,
        connect::{HttpConnector, dns::Name},
    },
    rt::TokioExecutor,
};

use crate::error::{AppError, AppResult};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

/// Whether requests may reach `ip`.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            !(v4.is_unspecified()
                || v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                // Carrier-grade NAT, 100.64.0.0/10.
                || (a == 100 && (64..128).contains(&b))
                // IETF protocol assignments, 192.0.0.0/24.
                || (a == 192 && b == 0 && c == 0)
                // Benchmarking, 198.18.0.0/15.
                || (a == 198 && (b == 18 || b == 19))
                || a >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments().first().copied().unwrap_or(0);
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                // Unique local fc00::/7 and link-local fe80::/10.
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                // NAT64 64:ff9b::/96 can reach IPv4 addresses.
                || (first == 0x0064 && v6.segments().get(1) == Some(&0xff9b)))
        }
    }
}

/// Resolves names and drops internal addresses unless they are allowed.
#[derive(Clone)]
struct Guard {
    allow_private: bool,
}

type Resolved = std::vec::IntoIter<SocketAddr>;

impl tower_service::Service<Name> for Guard {
    type Response = Resolved;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Resolved, std::io::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let allow_private = self.allow_private;
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let found: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            let allowed: Vec<SocketAddr> = found
                .iter()
                .copied()
                .filter(|address| allow_private || is_public(address.ip()))
                .collect();
            if allowed.is_empty() && !found.is_empty() {
                return Err(std::io::Error::other(format!(
                    "{host} is an internal address; an admin can allow those in the automation settings"
                )));
            }
            Ok(allowed.into_iter())
        })
    }
}

type HttpsClient = Client<HttpsConnector<HttpConnector<Guard>>, Full<Bytes>>;

/// What a script asked to send.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub elapsed: Duration,
}

/// The HTTP client scripts share.
pub struct Http {
    client: HttpsClient,
    runtime: tokio::runtime::Handle,
    allow_private: bool,
    user_agent: String,
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Http")
            .field("allow_private", &self.allow_private)
            .finish_non_exhaustive()
    }
}

impl Http {
    /// Needs to be called inside the Tokio runtime it will use.
    pub fn new(allow_private: bool) -> AppResult<Self> {
        let mut http = HttpConnector::new_with_resolver(Guard { allow_private });
        http.enforce_http(false);
        http.set_connect_timeout(Some(DEFAULT_TIMEOUT));
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .map_err(AppError::internal)?
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .wrap_connector(http);
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            runtime: tokio::runtime::Handle::try_current().map_err(AppError::internal)?,
            allow_private,
            user_agent: format!(
                "Sideporch/{} (+https://github.com/niklas-heer/sideporch)",
                env!("CARGO_PKG_VERSION")
            ),
        })
    }

    /// Sends `request` and waits for the whole response. Must not be called
    /// from an async task.
    pub fn send(&self, request: Request) -> Result<Response, String> {
        let uri: axum::http::Uri = request
            .url
            .parse()
            .map_err(|_| format!("`{}` is not a valid URL", request.url))?;
        if !matches!(uri.scheme_str(), Some("http" | "https")) {
            return Err("only http and https URLs are allowed".to_owned());
        }
        let host = uri.host().ok_or("the URL needs a host")?;
        // Literal IP addresses skip name resolution, so check them here.
        if let Ok(ip) = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            && !self.allow_private
            && !is_public(ip)
        {
            return Err(format!(
                "{host} is an internal address; an admin can allow those in the automation settings"
            ));
        }
        if request.body.len() > MAX_REQUEST_BYTES {
            return Err("the request body is larger than 1 MB".to_owned());
        }
        let method: axum::http::Method = request
            .method
            .to_uppercase()
            .parse()
            .map_err(|_| format!("`{}` is not an HTTP method", request.method))?;
        let mut builder = axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("user-agent", &self.user_agent);
        for (name, value) in &request.headers {
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "host" | "content-length" | "transfer-encoding" | "connection"
            ) {
                continue;
            }
            builder = builder.header(name.as_str(), value.as_str());
        }
        let outgoing = builder
            .body(Full::new(Bytes::from(request.body)))
            .map_err(|error| format!("invalid request: {error}"))?;
        let timeout = request.timeout.min(MAX_TIMEOUT);
        let started = Instant::now();
        let client = self.client.clone();
        self.runtime.block_on(async move {
            let exchange = async {
                let response = client
                    .request(outgoing)
                    .await
                    .map_err(|error| describe(&error))?;
                let (parts, body) = response.into_parts();
                let bytes = Limited::new(body, MAX_RESPONSE_BYTES)
                    .collect()
                    .await
                    .map_err(|_| "the response is larger than 5 MB".to_owned())?
                    .to_bytes();
                let headers = parts
                    .headers
                    .iter()
                    .filter_map(|(name, value)| {
                        Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
                    })
                    .collect();
                Ok(Response {
                    status: parts.status.as_u16(),
                    headers,
                    body: String::from_utf8_lossy(&bytes).into_owned(),
                    elapsed: started.elapsed(),
                })
            };
            tokio::time::timeout(timeout, exchange)
                .await
                .map_err(|_| format!("no answer within {} seconds", timeout.as_secs()))?
        })
    }
}

impl Http {
    /// Requests `url` with GET, following up to five redirects.
    async fn follow(
        &self,
        url: &str,
    ) -> Result<axum::http::Response<hyper::body::Incoming>, String> {
        let mut url = url.to_owned();
        let mut response = None;
        for _ in 0..6 {
            let uri: axum::http::Uri = url
                .parse()
                .map_err(|_| format!("`{url}` is not a valid URL"))?;
            if !matches!(uri.scheme_str(), Some("http" | "https")) {
                return Err("only http and https URLs are allowed".to_owned());
            }
            // Literal IP addresses skip name resolution, so check them here.
            if let Some(host) = uri.host()
                && let Ok(ip) = host
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .parse::<IpAddr>()
                && !self.allow_private
                && !is_public(ip)
            {
                return Err(format!("{host} is an internal address"));
            }
            let request = axum::http::Request::get(uri.clone())
                .header("user-agent", &self.user_agent)
                .body(Full::new(Bytes::new()))
                .map_err(|error| format!("invalid request: {error}"))?;
            let answer = tokio::time::timeout(Duration::from_mins(1), self.client.request(request))
                .await
                .map_err(|_| "the server didn't answer within a minute".to_owned())?
                .map_err(|error| describe(&error))?;
            if answer.status().is_redirection() {
                let location = answer
                    .headers()
                    .get(axum::http::header::LOCATION)
                    .and_then(|value| value.to_str().ok())
                    .ok_or("a redirect without a location")?;
                url = if location.starts_with("http://") || location.starts_with("https://") {
                    location.to_owned()
                } else {
                    let origin = format!(
                        "{}://{}",
                        uri.scheme_str().unwrap_or("https"),
                        uri.authority().map(ToString::to_string).unwrap_or_default()
                    );
                    format!("{origin}{location}")
                };
                continue;
            }
            response = Some(answer);
            break;
        }
        response.ok_or_else(|| "too many redirects".to_owned())
    }

    /// Downloads `url` to `target`, following up to five redirects, and
    /// checks the file's size and SHA-256 before moving it into place.
    /// `progress` counts the bytes received.
    pub async fn download(
        &self,
        url: &str,
        target: &std::path::Path,
        size: u64,
        sha256: &str,
        progress: &std::sync::atomic::AtomicU64,
    ) -> Result<(), String> {
        use sha2::Digest as _;
        use tokio::io::AsyncWriteExt as _;

        let response = self.follow(url).await?;
        if !response.status().is_success() {
            return Err(format!("the server answered {}", response.status()));
        }
        let partial = target.with_extension("partial");
        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| error.to_string())?;
        }
        let mut file = tokio::fs::File::create(&partial)
            .await
            .map_err(|error| error.to_string())?;
        let mut hasher = sha2::Sha256::new();
        let mut received = 0_u64;
        let mut body = response.into_body();
        loop {
            let frame = tokio::time::timeout(Duration::from_mins(1), body.frame())
                .await
                .map_err(|_| "the download stalled for a minute".to_owned())?;
            let Some(frame) = frame else { break };
            let frame = frame.map_err(|error| describe(&error))?;
            if let Ok(data) = frame.into_data() {
                received = received.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
                if received > size {
                    drop(tokio::fs::remove_file(&partial).await);
                    return Err("the file is larger than expected".to_owned());
                }
                hasher.update(&data);
                file.write_all(&data)
                    .await
                    .map_err(|error| error.to_string())?;
                progress.fetch_add(
                    u64::try_from(data.len()).unwrap_or(0),
                    std::sync::atomic::Ordering::Relaxed,
                );
            }
        }
        file.flush().await.map_err(|error| error.to_string())?;
        drop(file);
        let digest = hasher.finalize();
        let hex = digest.iter().fold(String::new(), |mut hex, byte| {
            use std::fmt::Write as _;
            // Writing to a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        });
        if received != size || hex != sha256 {
            drop(tokio::fs::remove_file(&partial).await);
            return Err("the downloaded file doesn't match; try again".to_owned());
        }
        tokio::fs::rename(&partial, target)
            .await
            .map_err(|error| error.to_string())
    }
}

/// The innermost cause, which names the actual problem.
fn describe(error: &(dyn std::error::Error + 'static)) -> String {
    let mut cause: &(dyn std::error::Error + 'static) = error;
    while let Some(source) = cause.source() {
        cause = source;
    }
    cause.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_internal_addresses() {
        for internal in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
        ] {
            assert!(!is_public(internal.parse().unwrap()), "{internal}");
        }
        for public in ["1.1.1.1", "140.82.112.3", "2606:4700::1111"] {
            assert!(is_public(public.parse().unwrap()), "{public}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn downloads_follow_redirects_and_check_the_file() {
        use sha2::Digest as _;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new()
            .route(
                "/moved",
                axum::routing::get(|| async { axum::response::Redirect::temporary("/file") }),
            )
            .route("/file", axum::routing::get(|| async { "model weights" }));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("models/weights.onnx");
        let sha =
            sha2::Sha256::digest(b"model weights")
                .iter()
                .fold(String::new(), |mut hex, byte| {
                    use std::fmt::Write as _;
                    write!(hex, "{byte:02x}").unwrap();
                    hex
                });
        let progress = std::sync::atomic::AtomicU64::new(0);
        let http = Http::new(true).unwrap();
        http.download(&format!("{base}/moved"), &target, 13, &sha, &progress)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"model weights");
        assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), 13);
        // A different file, or a bigger one, is refused and removed.
        let other = dir.path().join("other.onnx");
        let error = http
            .download(
                &format!("{base}/file"),
                &other,
                13,
                &"0".repeat(64),
                &progress,
            )
            .await
            .unwrap_err();
        assert!(error.contains("doesn't match"), "{error}");
        let error = http
            .download(&format!("{base}/file"), &other, 5, &sha, &progress)
            .await
            .unwrap_err();
        assert!(error.contains("larger than expected"), "{error}");
        assert!(!other.exists() && !other.with_extension("partial").exists());
        // Internal addresses stay blocked unless allowed.
        let guarded = Http::new(false).unwrap();
        assert!(
            guarded
                .download(&format!("{base}/file"), &other, 13, &sha, &progress)
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refuses_internal_targets_unless_allowed() {
        let http = Http::new(false).unwrap();
        let blocked = tokio::task::spawn_blocking(move || {
            let request = |url: &str| Request {
                method: "GET".to_owned(),
                url: url.to_owned(),
                headers: Vec::new(),
                body: Vec::new(),
                timeout: Duration::from_secs(2),
            };
            (
                http.send(request("http://127.0.0.1:9/")),
                http.send(request("http://localhost:9/")),
                http.send(request("ftp://example.com/")),
            )
        })
        .await
        .unwrap();
        assert!(blocked.0.unwrap_err().contains("internal address"));
        // Build sandboxes may have no resolver; then the name cannot resolve at all.
        let resolvable = std::net::ToSocketAddrs::to_socket_addrs("localhost:9").is_ok();
        if resolvable {
            assert!(blocked.1.unwrap_err().contains("internal address"));
        } else {
            assert!(blocked.1.is_err());
        }
        assert!(blocked.2.unwrap_err().contains("only http and https"));
    }
}
