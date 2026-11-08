//! Typing between servers: a best-effort notice that is never queued or
//! stored.
//!
//! When someone writes in a shared channel, the servers it is shared with
//! hear about it at most every couple of seconds. A notice is signed like
//! every request between servers, sent once with a short timeout, and then
//! forgotten: no outbox, no table, no retries. A notice that is lost or
//! late only means the dots show late or not at all. Notices follow the
//! topology of messages (see [`super::outbound::targets`]): a host passes a
//! guest's notice on to its other guests, a guest tells only its host, and
//! nothing goes back the way it came.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    hash::{DefaultHasher, Hash as _, Hasher as _},
    sync::Arc,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{
    data::{self, Own, Status},
    events::Person,
    outbound,
};
use crate::{AppState, error::AppResult, realtime};

/// The path other servers post notices to.
pub const PATH: &str = "/federation/typing";
/// The largest notice accepted, in bytes.
pub const MAX_BYTES: usize = 4096;
/// How long a notice may take to reach a server before it is dropped.
const TIMEOUT: Duration = Duration::from_secs(2);
/// The shortest gap between two notices for the same person, channel,
/// thread and server. Browsers send at most every 3 seconds; this only
/// keeps several open tabs from multiplying them.
const SEND_GAP: Duration = Duration::from_secs(2);
/// How many notices one server may send us in a window.
const RECEIVE_MAX: u32 = 100;
const RECEIVE_WINDOW: Duration = Duration::from_secs(10);
/// How long a request's nonce is remembered. Requests whose date is
/// further off than the clock skew are refused anyway.
const NONCE_MEMORY: Duration = Duration::from_mins(10);
/// How many sends to remember before forgetting the ones out of the gap.
const REMEMBERED_SENDS: usize = 10_000;

/// A notice that someone is writing, or has stopped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notice {
    /// The channel's id on its host, as in other events.
    pub channel: i64,
    /// The uid of the thread's first message, for a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Who is writing, on their own server.
    pub person: Person,
    #[serde(default)]
    pub stopped: bool,
}

/// Why a verified notice was not let through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The server has sent too many notices lately.
    Busy,
    /// The request's nonce was seen before.
    Replayed,
}

/// What this server remembers about typing notices. It lives in memory: a
/// restart forgets it, which costs at most a few notices.
#[derive(Default)]
pub struct Gate {
    /// When each person last started typing toward each server.
    sent: HashMap<(String, i64, Option<String>, i64), Instant>,
    /// Per sending server: when its window opened, and how many it sent.
    received: HashMap<i64, (Instant, u32)>,
    nonces: HashSet<(i64, String)>,
    nonce_times: VecDeque<(Instant, (i64, String))>,
}

impl Gate {
    /// Whether a notice may go out now, and records it if so.
    fn may_send(&mut self, key: (String, i64, Option<String>, i64), now: Instant) -> bool {
        if self
            .sent
            .get(&key)
            .is_some_and(|last| now.duration_since(*last) < SEND_GAP)
        {
            return false;
        }
        if self.sent.len() >= REMEMBERED_SENDS {
            self.sent
                .retain(|_, last| now.duration_since(*last) < SEND_GAP);
        }
        self.sent.insert(key, now);
        true
    }

    /// Lets a verified notice from `server` through, unless it is a replay
    /// or `server` is over its limit. Records what it lets through.
    pub fn admit(&mut self, server: i64, nonce: &str, now: Instant) -> Result<(), Refused> {
        let window = self.received.entry(server).or_insert((now, 0));
        if now.duration_since(window.0) >= RECEIVE_WINDOW {
            *window = (now, 0);
        }
        if window.1 >= RECEIVE_MAX {
            return Err(Refused::Busy);
        }
        window.1 = window.1.saturating_add(1);
        while self
            .nonce_times
            .front()
            .is_some_and(|(seen, _)| now.duration_since(*seen) >= NONCE_MEMORY)
        {
            if let Some((_, key)) = self.nonce_times.pop_front() {
                self.nonces.remove(&key);
            }
        }
        let key = (server, nonce.to_owned());
        if !self.nonces.insert(key.clone()) {
            return Err(Refused::Replayed);
        }
        self.nonce_times.push_back((now, key));
        Ok(())
    }
}

/// The name a person's typing is throttled under, the same on every server.
fn who(person: &Person) -> String {
    format!("{}#{}", person.server.to_ascii_lowercase(), person.id)
}

/// The id the browser knows a person from another server by. It is
/// negative, so it never matches an account here, and the same on every
/// request.
pub fn typist_id(person: &Person) -> i64 {
    let mut hasher = DefaultHasher::new();
    who(person).hash(&mut hasher);
    i64::try_from(hasher.finish() & 0x7FFF_FFFF_FFFF_FFFF)
        .ok()
        .and_then(i64::checked_neg)
        .unwrap_or(i64::MIN)
}

/// After `user_id` started (or stopped) typing in `channel_id`, here, or in
/// a thread there whose first message is `parent_id`.
pub async fn announce(
    state: &AppState,
    channel_id: i64,
    user_id: i64,
    parent_id: Option<i64>,
    stopped: bool,
) {
    let (Some(handle), Some(url)) = (
        state.federation.handle(),
        state.federation.url().map(ToOwned::to_owned),
    ) else {
        return;
    };
    let found = state
        .db
        .call(move |conn| {
            let own = Own {
                handle: &handle,
                url: &url,
            };
            let Some(person) = data::person(conn, &own, user_id)? else {
                return Ok(None);
            };
            let parent = match parent_id {
                Some(id) => Some(outbound::uid_of(conn, &own, id)?),
                None => None,
            };
            Ok(Some((person, parent)))
        })
        .await;
    match found {
        Ok(Some((person, parent))) => relay(state, channel_id, None, person, parent, stopped).await,
        Ok(None) => {}
        Err(error) => tracing::warn!(%error, "could not tell other servers about typing"),
    }
}

/// Passes a notice about `local` (our id for the channel) on to the servers
/// it is shared with, except `from`, the way messages go. Returns at once:
/// each send runs on its own and is given up after [`TIMEOUT`].
pub async fn relay(
    state: &AppState,
    local: i64,
    from: Option<i64>,
    person: Person,
    parent: Option<String>,
    stopped: bool,
) {
    let targets = state
        .db
        .call(move |conn| {
            let mut targets = Vec::new();
            for (instance_id, channel) in outbound::targets(conn, local, from)? {
                if let Some(instance) = data::instance(conn, instance_id)?
                    .filter(|instance| instance.status == Status::Connected)
                {
                    targets.push((instance.id, instance.url, channel));
                }
            }
            Ok(targets)
        })
        .await;
    let targets = match targets {
        Ok(targets) => targets,
        Err(error) => {
            tracing::warn!(%error, "could not tell other servers about typing");
            return;
        }
    };
    let now = Instant::now();
    for (instance, url, channel) in targets {
        // Stopping is never held back: a stop only follows a start.
        if !stopped {
            let key = (who(&person), local, parent.clone(), instance);
            if !state.federation.typing_gate().may_send(key, now) {
                continue;
            }
        }
        let notice = Notice {
            channel,
            parent: parent.clone(),
            person: person.clone(),
            stopped,
        };
        send(Arc::clone(&state.federation), url, notice);
    }
}

/// Sends one notice in the background, once, and forgets it.
fn send(federation: super::Shared, url: String, notice: Notice) {
    tokio::spawn(async move {
        let Ok(body) = serde_json::to_vec(&notice) else {
            return;
        };
        match tokio::time::timeout(TIMEOUT, federation.send("POST", &url, PATH, body)).await {
            Ok(Ok((status, _))) if (200..300).contains(&status) => {}
            Ok(Ok((status, _))) => {
                tracing::debug!(%url, status, "another server didn't take a typing notice");
            }
            Ok(Err(error)) => tracing::debug!(%url, %error, "could not send a typing notice"),
            Err(_) => tracing::debug!(%url, "a typing notice timed out"),
        }
    });
}

/// The account standing in for `person` here, if there is one. Unlike
/// [`data::account`], this never makes one.
pub fn existing_account(conn: &rusqlite::Connection, person: &Person) -> AppResult<Option<i64>> {
    use rusqlite::OptionalExtension as _;
    let Some(instance) = data::instance_by_handle(conn, &person.server)? else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT id FROM users WHERE instance_id = ?1 AND remote_id = ?2 AND deactivated_at IS NULL",
            rusqlite::params![instance.id, person.id],
            |row| row.get(0),
        )
        .optional()?)
}

/// What the realtime event for a notice says, once checked.
pub fn event(local: i64, parent: Option<i64>, person: &Person, stopped: bool) -> realtime::Event {
    realtime::Event::Typing {
        channel_id: local,
        parent_id: parent,
        user_id: typist_id(person),
        name: person.display_name.chars().take(64).collect(),
        stopped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(server: &str, id: i64) -> Person {
        Person {
            server: server.to_owned(),
            url: format!("https://{server}"),
            id,
            username: "bea".to_owned(),
            display_name: "Bea".to_owned(),
        }
    }

    #[test]
    fn sends_to_one_server_once_per_gap() {
        let mut gate = Gate::default();
        let start = Instant::now();
        let key = || ("b#1".to_owned(), 7, None, 3);
        assert!(gate.may_send(key(), start));
        assert!(!gate.may_send(key(), start + Duration::from_millis(1900)));
        assert!(gate.may_send(key(), start + SEND_GAP));
        // Another thread or server is its own gap.
        assert!(gate.may_send(("b#1".to_owned(), 7, Some("a#2".to_owned()), 3), start));
        assert!(gate.may_send(("b#1".to_owned(), 7, None, 4), start));
    }

    #[test]
    fn a_server_is_limited_per_window() {
        let mut gate = Gate::default();
        let start = Instant::now();
        for n in 0..RECEIVE_MAX {
            assert_eq!(gate.admit(1, &n.to_string(), start), Ok(()));
        }
        assert_eq!(
            gate.admit(1, "over", start),
            Err(Refused::Busy),
            "over the limit"
        );
        // Another server has its own allowance.
        assert_eq!(gate.admit(2, "other", start), Ok(()));
        assert_eq!(
            gate.admit(1, "later", start + RECEIVE_WINDOW),
            Ok(()),
            "a new window"
        );
    }

    #[test]
    fn a_nonce_is_refused_twice_until_it_is_forgotten() {
        let mut gate = Gate::default();
        let start = Instant::now();
        assert_eq!(gate.admit(1, "abc", start), Ok(()));
        assert_eq!(
            gate.admit(1, "abc", start + Duration::from_secs(1)),
            Err(Refused::Replayed)
        );
        // The same nonce from another server is a different request.
        assert_eq!(gate.admit(2, "abc", start), Ok(()));
        assert_eq!(
            gate.admit(1, "abc", start + NONCE_MEMORY),
            Ok(()),
            "forgotten after the memory"
        );
    }

    #[test]
    fn remote_typists_are_negative_and_stable() {
        let bea = person("b.example", 7);
        assert!(typist_id(&bea) < 0);
        assert_eq!(typist_id(&bea), typist_id(&person("B.example", 7)));
        assert_ne!(typist_id(&bea), typist_id(&person("b.example", 8)));
    }
}
