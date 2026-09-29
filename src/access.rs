//! Who connects: the client's address, limits on failed sign-ins and on
//! sign-ups, bans, and the addresses each account used lately.
//!
//! The address is the connection's peer, or, behind a proxy the admin
//! trusts, the header that proxy sets (`--client-ip-header`). Without that
//! setting headers are ignored, so nobody can claim another address.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Mutex, RwLock},
    time::{Duration, Instant},
};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request, State},
    http::{HeaderName, StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use rusqlite::{Connection, params};

use crate::{
    AppState,
    error::{AppError, AppResult},
    now_ms,
};

/// How long addresses are kept per account.
pub const KEEP_ADDRESSES: Duration = Duration::from_hours(24 * 30);
/// An account's address is written down at most this often.
const SEEN_EVERY: Duration = Duration::from_hours(24);

/// A limit: at most `max` events per `window`.
#[derive(Debug, Clone, Copy)]
pub struct Limit {
    pub max: usize,
    pub window: Duration,
}

/// Failed password sign-ins from one address.
pub const SIGN_IN_PER_ADDRESS: Limit = Limit {
    max: 10,
    window: Duration::from_mins(15),
};
/// Failed password sign-ins for one account, from anywhere. Higher than
/// per address, so others can't easily lock someone out.
pub const SIGN_IN_PER_ACCOUNT: Limit = Limit {
    max: 20,
    window: Duration::from_mins(15),
};
/// New accounts per address.
pub const SIGN_UP: Limit = Limit {
    max: 3,
    window: Duration::from_hours(1),
};

/// The most entries a table in memory keeps before starting over, so floods
/// can't use up memory.
const MAX_ENTRIES: usize = 100_000;

/// Leading zero bits a sign-up's proof of work needs: about a quarter of
/// a million hashes, a moment for a browser, and a cost for mass sign-ups.
pub const PROOF_BITS: u32 = 18;
/// How long a sign-up form's challenge stays good.
const CHALLENGE_LIFETIME: Duration = Duration::from_hours(1);

/// What a limit counts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Counted {
    FailedSignIn(IpAddr),
    FailedSignInAs(String),
    SignUp(IpAddr),
}

/// A range of addresses, such as `203.0.113.0/24` or one address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    network: IpAddr,
    prefix: u8,
}

impl Range {
    /// Reads `203.0.113.7`, `203.0.113.0/24` or `2001:db8::/48`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix.parse::<u8>().ok()?)),
            None => (text, None),
        };
        let network = normalize(address.parse().ok()?);
        let bits = if network.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(bits);
        (prefix <= bits).then_some(Self { network, prefix })
    }

    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.network, normalize(address)) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                let mask = u32::MAX
                    .checked_shl(32_u32.saturating_sub(u32::from(self.prefix)))
                    .unwrap_or(0);
                u32::from(network) & mask == u32::from(address) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                let mask = u128::MAX
                    .checked_shl(128_u32.saturating_sub(u32::from(self.prefix)))
                    .unwrap_or(0);
                u128::from(network) & mask == u128::from(address) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for Range {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bits = if self.network.is_ipv4() { 32 } else { 128 };
        if self.prefix == bits {
            write!(f, "{}", self.network)
        } else {
            write!(f, "{}/{}", self.network, self.prefix)
        }
    }
}

/// IPv4 addresses arrive as `::ffff:a.b.c.d` on dual-stack sockets.
fn normalize(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 @ IpAddr::V4(_) => v4,
    }
}

/// What a ban names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Addresses(Range),
    /// A whole address, lower case.
    Email(String),
    /// Every address at a domain, lower case, without the `@`.
    Domain(String),
}

impl Target {
    /// Reads what a moderator typed: an address or range, `name@domain`,
    /// or `@domain`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_lowercase();
        if let Some(domain) = text.strip_prefix('@') {
            return (!domain.is_empty() && domain.contains('.') && !domain.contains('@'))
                .then(|| Self::Domain(domain.to_owned()));
        }
        if text.contains('@') {
            return Some(Self::Email(text));
        }
        Range::parse(&text).map(Self::Addresses)
    }

    const fn kind(&self) -> &'static str {
        match self {
            Self::Addresses(_) => "address",
            Self::Email(_) | Self::Domain(_) => "email",
        }
    }

    fn value(&self) -> String {
        match self {
            Self::Addresses(range) => range.to_string(),
            Self::Email(email) => email.clone(),
            Self::Domain(domain) => format!("@{domain}"),
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.value())
    }
}

#[derive(Debug, Clone)]
pub struct Ban {
    pub id: i64,
    pub target: Target,
    pub reason: String,
    pub created_at: i64,
    pub created_by: Option<String>,
    /// `None` for bans that don't end.
    pub expires_at: Option<i64>,
}

impl Ban {
    fn active(&self, now: i64) -> bool {
        self.expires_at.is_none_or(|end| end > now)
    }
}

/// Everything this module keeps between requests.
pub struct Access {
    /// The header a trusted proxy puts the client's address in.
    header: Option<HeaderName>,
    counted: Mutex<HashMap<Counted, Vec<Instant>>>,
    bans: RwLock<Vec<Ban>>,
    /// When each account's address was last written down.
    seen: Mutex<HashMap<(i64, IpAddr), Instant>>,
    /// Proof-of-work challenges handed out with sign-up forms.
    challenges: Mutex<HashMap<String, Instant>>,
}

impl Access {
    pub fn new(header: Option<&str>) -> AppResult<Self> {
        let header = header
            .map(|name| {
                HeaderName::try_from(name.trim())
                    .map_err(|_| AppError::internal(format!("`{name}` is not a header name")))
            })
            .transpose()?;
        Ok(Self {
            header,
            counted: Mutex::default(),
            bans: RwLock::default(),
            seen: Mutex::default(),
            challenges: Mutex::default(),
        })
    }

    /// A new proof-of-work challenge for a sign-up form.
    pub fn challenge(&self) -> AppResult<String> {
        let challenge = crate::auth::random_token()?;
        if let Ok(mut challenges) = self.challenges.lock() {
            let now = Instant::now();
            if challenges.len() > 10_000 {
                challenges.retain(|_, at| now.duration_since(*at) < CHALLENGE_LIFETIME);
                // Someone is fetching forms by the thousand; open forms get
                // a fresh challenge when they're sent.
                if challenges.len() > MAX_ENTRIES {
                    challenges.clear();
                }
            }
            challenges.insert(challenge.clone(), now);
        }
        Ok(challenge)
    }

    /// Whether `nonce` solves `challenge`, which then can't be used again.
    pub fn redeem(&self, challenge: &str, nonce: &str) -> bool {
        let Ok(mut challenges) = self.challenges.lock() else {
            return false;
        };
        let fresh = challenges
            .get(challenge)
            .is_some_and(|at| at.elapsed() < CHALLENGE_LIFETIME);
        if !fresh || nonce.len() > 32 || !solves(challenge, nonce, PROOF_BITS) {
            return false;
        }
        challenges.remove(challenge);
        true
    }

    /// The client's address for a request.
    pub fn client_ip(&self, parts: &Parts) -> Option<IpAddr> {
        if let Some(header) = &self.header {
            // X-Forwarded-For lists the client first, then each proxy.
            return parts
                .headers
                .get(header)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(',').next())
                .and_then(|first| first.trim().parse().ok())
                .map(normalize);
        }
        parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| normalize(info.0.ip()))
    }

    /// `Err` with how long to wait when `what` reached `limit`.
    pub fn check(&self, what: &Counted, limit: Limit) -> Result<(), Duration> {
        let Ok(mut counted) = self.counted.lock() else {
            return Ok(());
        };
        let now = Instant::now();
        let Some(times) = counted.get_mut(what) else {
            return Ok(());
        };
        times.retain(|at| now.duration_since(*at) < limit.window);
        if times.len() < limit.max {
            return Ok(());
        }
        let oldest = times.iter().min().copied().unwrap_or(now);
        Err(limit.window.saturating_sub(now.duration_since(oldest)))
    }

    /// Counts one more `what`.
    pub fn count(&self, what: Counted) {
        if let Ok(mut counted) = self.counted.lock() {
            // Forget stale entries now and then, so the map stays small.
            if counted.len() > 10_000 {
                let now = Instant::now();
                counted.retain(|_, times| {
                    times.retain(|at| now.duration_since(*at) < Duration::from_hours(1));
                    !times.is_empty()
                });
                // Many addresses at once, as IPv6 makes easy: start over
                // rather than grow without end.
                if counted.len() > MAX_ENTRIES {
                    counted.clear();
                }
            }
            counted.entry(what).or_default().push(Instant::now());
        }
    }

    /// Forgets `what`, such as failed sign-ins after one succeeds.
    pub fn forget(&self, what: &Counted) {
        if let Ok(mut counted) = self.counted.lock() {
            counted.remove(what);
        }
    }

    /// The active ban covering `address`, if any.
    pub fn address_ban(&self, address: IpAddr) -> Option<Ban> {
        let now = now_ms();
        self.bans
            .read()
            .ok()?
            .iter()
            .find(|ban| {
                ban.active(now)
                    && matches!(&ban.target, Target::Addresses(range) if range.contains(address))
            })
            .cloned()
    }

    /// Whether an email address may not be used here.
    pub fn email_banned(&self, email: &str) -> bool {
        let email = email.trim().to_lowercase();
        let domain = email.rsplit_once('@').map(|(_, domain)| domain);
        let now = now_ms();
        self.bans.read().is_ok_and(|bans| {
            bans.iter().any(|ban| {
                ban.active(now)
                    && match &ban.target {
                        Target::Email(banned) => *banned == email,
                        Target::Domain(banned) => domain.is_some_and(|domain| {
                            domain == banned || domain.ends_with(&format!(".{banned}"))
                        }),
                        Target::Addresses(_) => false,
                    }
            })
        })
    }

    /// Reads the bans again after a change.
    pub async fn reload(&self, state: &AppState) -> AppResult<()> {
        let bans = state.db.call(|conn| bans(conn)).await?;
        if let Ok(mut current) = self.bans.write() {
            *current = bans;
        }
        Ok(())
    }

    /// Whether `user_id`'s `address` is due to be written down.
    fn due(&self, user_id: i64, address: IpAddr) -> bool {
        let Ok(mut seen) = self.seen.lock() else {
            return false;
        };
        let now = Instant::now();
        if seen.len() > 50_000 {
            seen.retain(|_, at| now.duration_since(*at) < SEEN_EVERY);
            if seen.len() > MAX_ENTRIES {
                seen.clear();
            }
        }
        match seen.get(&(user_id, address)) {
            Some(at) if now.duration_since(*at) < SEEN_EVERY => false,
            _ => {
                seen.insert((user_id, address), now);
                true
            }
        }
    }
}

/// Whether SHA-256 of `challenge:nonce` starts with `bits` zero bits.
pub fn solves(challenge: &str, nonce: &str, bits: u32) -> bool {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(format!("{challenge}:{nonce}").as_bytes());
    let mut zeros: u32 = 0;
    for byte in digest {
        if byte == 0 {
            zeros = zeros.saturating_add(8);
        } else {
            zeros = zeros.saturating_add(byte.leading_zeros());
            break;
        }
        if zeros >= bits {
            break;
        }
    }
    zeros >= bits
}

/// The client's address, as the server sees it.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub Option<IpAddr>);

impl FromRequestParts<AppState> for ClientIp {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(state.access.client_ip(parts)))
    }
}

/// Refuses every request from a banned address, except the health check.
pub async fn refuse_banned(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let (parts, body) = request.into_parts();
    let banned = parts.uri.path() != "/healthz"
        && state
            .access
            .client_ip(&parts)
            .and_then(|address| state.access.address_ban(address))
            .is_some();
    if banned {
        return (
            StatusCode::FORBIDDEN,
            crate::views::error_page(
                StatusCode::FORBIDDEN,
                "This server doesn't accept visits from your network. If you think that's a mistake, contact its admins.",
            ),
        )
            .into_response();
    }
    next.run(Request::from_parts(parts, body)).await
}

/// Writes down that `user_id` used `address`, at most once a day.
pub fn seen(state: &AppState, user_id: i64, address: Option<IpAddr>) {
    let Some(address) = address else {
        return;
    };
    if !state.access.due(user_id, address) {
        return;
    }
    let db = state.db.clone();
    tokio::spawn(async move {
        let now = now_ms();
        let saved = db
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO addresses (user_id, ip, first_seen, last_seen) VALUES (?1, ?2, ?3, ?3)
                     ON CONFLICT (user_id, ip) DO UPDATE SET last_seen = excluded.last_seen",
                    params![user_id, address.to_string(), now],
                )?;
                Ok(())
            })
            .await;
        if let Err(error) = saved {
            tracing::warn!(?error, "could not note an address");
        }
    });
}

/// An address someone used, and when.
#[derive(Debug, Clone)]
pub struct Address {
    pub ip: String,
    pub first_seen: i64,
    pub last_seen: i64,
    /// Other accounts that used it, by display name.
    pub shared_with: Vec<(i64, String)>,
}

/// The addresses `user_id` used lately, newest first.
pub fn addresses(conn: &Connection, user_id: i64) -> AppResult<Vec<Address>> {
    let mut statement = conn.prepare(
        "SELECT ip, first_seen, last_seen FROM addresses WHERE user_id = ?1 ORDER BY last_seen DESC LIMIT 20",
    )?;
    let rows: Vec<(String, i64, i64)> = statement
        .query_map([user_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<_, _>>()?;
    let mut others = conn.prepare(
        "SELECT u.id, u.display_name FROM addresses a JOIN users u ON u.id = a.user_id
         WHERE a.ip = ?1 AND a.user_id != ?2 ORDER BY u.display_name LIMIT 10",
    )?;
    rows.into_iter()
        .map(|(ip, first_seen, last_seen)| {
            let shared_with = others
                .query_map(params![ip, user_id], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            Ok(Address {
                ip,
                first_seen,
                last_seen,
                shared_with,
            })
        })
        .collect()
}

/// Every ban, newest first; ended ones too, until they're cleaned up.
pub fn bans(conn: &Connection) -> AppResult<Vec<Ban>> {
    let mut statement = conn.prepare(
        "SELECT b.id, b.value, b.reason, b.created_at, u.display_name, b.expires_at
         FROM bans b LEFT JOIN users u ON u.id = b.created_by ORDER BY b.id DESC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<i64>>(5)?,
        ))
    })?;
    let mut bans = Vec::new();
    for row in rows {
        let (id, value, reason, created_at, created_by, expires_at) = row?;
        if let Some(target) = Target::parse(&value) {
            bans.push(Ban {
                id,
                target,
                reason,
                created_at,
                created_by,
                expires_at,
            });
        }
    }
    Ok(bans)
}

pub fn add_ban(
    conn: &Connection,
    target: &Target,
    reason: &str,
    by: i64,
    expires_at: Option<i64>,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO bans (kind, value, reason, created_by, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![target.kind(), target.value(), reason.trim(), by, now_ms(), expires_at],
    )?;
    Ok(())
}

pub fn lift_ban(conn: &Connection, id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM bans WHERE id = ?1", [id])?;
    Ok(())
}

/// Forgets addresses older than [`KEEP_ADDRESSES`] and bans that ended.
pub fn clean_up(conn: &Connection) -> AppResult<()> {
    let now = now_ms();
    let keep = i64::try_from(KEEP_ADDRESSES.as_millis()).unwrap_or(i64::MAX);
    conn.execute(
        "DELETE FROM addresses WHERE last_seen < ?1",
        [now.saturating_sub(keep)],
    )?;
    conn.execute(
        "DELETE FROM bans WHERE expires_at IS NOT NULL AND expires_at <= ?1",
        [now],
    )?;
    Ok(())
}

/// Cleans up every hour.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        let mut hourly = tokio::time::interval(Duration::from_hours(1));
        loop {
            hourly.tick().await;
            if let Err(error) = state.db.call(|conn| clean_up(conn)).await {
                tracing::warn!(?error, "could not clean up addresses and bans");
            }
            if let Err(error) = state.access.reload(&state).await {
                tracing::warn!(?error, "could not read bans");
            }
        }
    });
}

/// Says how long to wait, for people who hit a limit.
pub fn wait_text(wait: Duration) -> String {
    let minutes = wait.as_secs().div_ceil(60).max(1);
    if minutes == 1 {
        "a minute".to_owned()
    } else {
        format!("{minutes} minutes")
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv6Addr;

    use super::*;

    #[test]
    fn ranges_match_their_addresses() {
        let range = Range::parse("203.0.113.0/24").expect("a range");
        assert!(range.contains("203.0.113.77".parse().expect("an address")));
        assert!(!range.contains("203.0.114.1".parse().expect("an address")));
        // IPv4 over an IPv6 socket.
        assert!(range.contains("::ffff:203.0.113.5".parse().expect("an address")));
        let one = Range::parse("198.51.100.7").expect("one address");
        assert!(one.contains("198.51.100.7".parse().expect("an address")));
        assert!(!one.contains("198.51.100.8".parse().expect("an address")));
        let v6 = Range::parse("2001:db8::/32").expect("a v6 range");
        assert!(v6.contains("2001:db8:1::5".parse().expect("an address")));
        assert!(!v6.contains(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert_eq!(Range::parse("10.0.0.0/33"), None);
        assert_eq!(v6.to_string(), "2001:db8::/32");
        assert!(
            Range::parse("0.0.0.0/0")
                .expect("everything")
                .contains("8.8.8.8".parse().expect("an address"))
        );
    }

    #[test]
    fn reads_what_moderators_type() {
        assert_eq!(
            Target::parse("@Spam.Example"),
            Some(Target::Domain("spam.example".to_owned()))
        );
        assert_eq!(
            Target::parse("Bot@Spam.example"),
            Some(Target::Email("bot@spam.example".to_owned()))
        );
        assert!(matches!(
            Target::parse("203.0.113.0/24"),
            Some(Target::Addresses(_))
        ));
        assert_eq!(Target::parse("not an address"), None);
        assert_eq!(Target::parse("@"), None);
    }

    #[test]
    fn email_bans_cover_the_address_and_domains_below_them() {
        let access = Access::new(None).expect("access");
        let ban = |target| Ban {
            id: 1,
            target,
            reason: String::new(),
            created_at: 0,
            created_by: None,
            expires_at: None,
        };
        if let Ok(mut bans) = access.bans.write() {
            bans.push(ban(Target::Domain("spam.example".to_owned())));
            bans.push(ban(Target::Email("bot@mail.example".to_owned())));
        }
        assert!(access.email_banned("Anyone@Spam.example"));
        assert!(access.email_banned("x@eu.spam.example"));
        assert!(!access.email_banned("x@notspam.example"));
        assert!(access.email_banned("bot@mail.example"));
        assert!(!access.email_banned("person@mail.example"));
    }

    #[test]
    fn limits_count_within_their_window() {
        let access = Access::new(None).expect("access");
        let what = Counted::FailedSignInAs("ada".to_owned());
        let limit = Limit {
            max: 2,
            window: Duration::from_mins(1),
        };
        assert!(access.check(&what, limit).is_ok());
        access.count(what.clone());
        access.count(what.clone());
        let wait = access.check(&what, limit).expect_err("limited");
        assert!(wait <= Duration::from_mins(1));
        access.forget(&what);
        assert!(access.check(&what, limit).is_ok());
    }
}
