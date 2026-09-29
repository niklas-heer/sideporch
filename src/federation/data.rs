//! What this server keeps about other servers.

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::{error::AppResult, store};

/// Where a connection with another server stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// We asked them to connect; their admin hasn't answered.
    Requested,
    /// They asked us; an admin here decides.
    Pending,
    Connected,
    Declined,
    Disconnected,
    /// Not connected: known because a shared channel's host passed on
    /// people from there.
    Relayed,
}

impl Status {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Pending => "pending",
            Self::Connected => "connected",
            Self::Declined => "declined",
            Self::Disconnected => "disconnected",
            Self::Relayed => "relayed",
        }
    }

    fn parse(key: &str) -> Self {
        match key {
            "requested" => Self::Requested,
            "pending" => Self::Pending,
            "connected" => Self::Connected,
            "declined" => Self::Declined,
            "relayed" => Self::Relayed,
            _ => Self::Disconnected,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub id: i64,
    /// Where it is, like `https://chat.example.org`.
    pub url: String,
    /// What people's names there end with: `chat.example.org`.
    pub handle: String,
    /// The name its admins gave it.
    pub name: String,
    /// Its signing key, pinned when we first connected.
    pub public_key: Option<Vec<u8>>,
    pub status: Status,
    /// What they wrote when asking to connect.
    pub note: String,
    /// People there may start direct conversations with people here.
    pub allow_direct: bool,
}

const COLUMNS: &str = "id, url, handle, name, public_key, status, note, allow_direct";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Instance> {
    Ok(Instance {
        id: row.get(0)?,
        url: row.get(1)?,
        handle: row.get(2)?,
        name: row.get(3)?,
        public_key: row.get(4)?,
        status: Status::parse(&row.get::<_, String>(5)?),
        note: row.get(6)?,
        allow_direct: row.get(7)?,
    })
}

pub fn instances(conn: &Connection) -> AppResult<Vec<Instance>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM instances WHERE status != 'relayed' ORDER BY name COLLATE NOCASE, handle"
    ))?;
    let instances = statement.query_map([], row)?.collect::<Result<_, _>>()?;
    Ok(instances)
}

pub fn instance(conn: &Connection, id: i64) -> AppResult<Option<Instance>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM instances WHERE id = ?1"),
            [id],
            row,
        )
        .optional()?)
}

pub fn instance_by_url(conn: &Connection, url: &str) -> AppResult<Option<Instance>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM instances WHERE url = ?1"),
            [url],
            row,
        )
        .optional()?)
}

pub fn instance_by_handle(conn: &Connection, handle: &str) -> AppResult<Option<Instance>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM instances WHERE handle = ?1"),
            [handle],
            row,
        )
        .optional()?)
}

/// What we learned about a server.
pub struct Seen<'a> {
    pub url: &'a str,
    pub handle: &'a str,
    pub name: &'a str,
    /// Kept as it was when `None`.
    pub public_key: Option<&'a [u8]>,
    pub status: Status,
    pub note: &'a str,
}

/// Remembers a server, or updates what we know about it. Returns its id.
pub fn save_instance(conn: &Connection, seen: &Seen<'_>, now: i64) -> AppResult<i64> {
    let Seen {
        url,
        handle,
        name,
        public_key,
        status,
        note,
    } = *seen;
    conn.execute(
        "INSERT INTO instances (url, handle, name, public_key, status, note, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
         ON CONFLICT (url) DO UPDATE SET handle = excluded.handle, name = excluded.name,
             public_key = COALESCE(excluded.public_key, instances.public_key),
             status = excluded.status, note = excluded.note, updated_at = excluded.updated_at",
        params![url, handle, name, public_key, status.key(), note, now],
    )?;
    Ok(
        conn.query_row("SELECT id FROM instances WHERE url = ?1", [url], |row| {
            row.get(0)
        })?,
    )
}

pub fn set_status(conn: &Connection, id: i64, status: Status, now: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE instances SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status.key(), now, id],
    )?;
    if matches!(status, Status::Disconnected | Status::Declined) {
        // Nothing is shared with a server that isn't connected, and nothing
        // is left to send it.
        conn.execute(
            "UPDATE shared_channels SET status = 'ended' WHERE instance_id = ?1",
            [id],
        )?;
        conn.execute("DELETE FROM channel_offers WHERE instance_id = ?1", [id])?;
        conn.execute("DELETE FROM federation_outbox WHERE instance_id = ?1", [id])?;
    }
    Ok(())
}

pub fn set_allow_direct(conn: &Connection, id: i64, allow: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE instances SET allow_direct = ?1 WHERE id = ?2",
        params![allow, id],
    )?;
    Ok(())
}

/// How many requests to connect wait for an admin.
pub fn pending_count(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM instances WHERE status = 'pending'",
        [],
        |row| row.get(0),
    )?)
}

/// Records a request's nonce. Returns false if `instance_id` sent it
/// before, which makes the request a replay.
pub fn remember_nonce(
    conn: &Connection,
    instance_id: i64,
    nonce: &str,
    now: i64,
) -> AppResult<bool> {
    // Older nonces fall outside the accepted clock skew anyway.
    conn.execute(
        "DELETE FROM federation_nonces WHERE seen_at < ?1",
        [now.saturating_sub(super::keys::CLOCK_SKEW_MS.saturating_mul(3))],
    )?;
    let added = conn.execute(
        "INSERT INTO federation_nonces (instance_id, nonce, seen_at) VALUES (?1, ?2, ?3)
         ON CONFLICT DO NOTHING",
        params![instance_id, nonce, now],
    )?;
    Ok(added > 0)
}

/// The name other servers see for this one.
pub fn team_name(conn: &Connection) -> AppResult<Option<String>> {
    Ok(store::setting(conn, "federation.name")?.filter(|name| !name.trim().is_empty()))
}

pub fn set_team_name(conn: &Connection, name: &str) -> AppResult<()> {
    store::set_setting(conn, "federation.name", name.trim())
}

/// This server's signing key, sealed with the secret key.
pub fn sealed_key(conn: &Connection) -> AppResult<Option<String>> {
    store::setting(conn, "federation.key")
}

pub fn set_sealed_key(conn: &Connection, sealed: &str) -> AppResult<()> {
    store::set_setting(conn, "federation.key", sealed)
}

// Shared channels

/// Which side of a shared channel this server is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The channel is ours; we pass everything on.
    Host,
    /// We keep a copy of another server's channel.
    Guest,
}

impl Role {
    const fn key(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Guest => "guest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareStatus {
    /// Offered; the guest's admin hasn't taken it yet.
    Invited,
    Active,
    Ended,
}

impl ShareStatus {
    const fn key(self) -> &'static str {
        match self {
            Self::Invited => "invited",
            Self::Active => "active",
            Self::Ended => "ended",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Share {
    pub channel_id: i64,
    pub instance_id: i64,
    pub role: Role,
    /// The channel's id on the other server.
    pub remote_channel_id: Option<i64>,
    pub status: ShareStatus,
}

fn share_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Share> {
    Ok(Share {
        channel_id: row.get(0)?,
        instance_id: row.get(1)?,
        role: if row.get::<_, String>(2)? == "host" {
            Role::Host
        } else {
            Role::Guest
        },
        remote_channel_id: row.get(3)?,
        status: match row.get::<_, String>(4)?.as_str() {
            "invited" => ShareStatus::Invited,
            "active" => ShareStatus::Active,
            _ => ShareStatus::Ended,
        },
    })
}

const SHARE_COLUMNS: &str = "channel_id, instance_id, role, remote_channel_id, status";

/// Every server `channel_id` is shared with, or offered to.
pub fn shares_of(conn: &Connection, channel_id: i64) -> AppResult<Vec<Share>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {SHARE_COLUMNS} FROM shared_channels WHERE channel_id = ?1"
    ))?;
    let shares = statement
        .query_map([channel_id], share_row)?
        .collect::<Result<_, _>>()?;
    Ok(shares)
}

pub fn share(conn: &Connection, channel_id: i64, instance_id: i64) -> AppResult<Option<Share>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {SHARE_COLUMNS} FROM shared_channels WHERE channel_id = ?1 AND instance_id = ?2"
            ),
            params![channel_id, instance_id],
            share_row,
        )
        .optional()?)
}

/// Our copy of the channel `instance_id` hosts as `remote_channel_id`.
pub fn guest_share(
    conn: &Connection,
    instance_id: i64,
    remote_channel_id: i64,
) -> AppResult<Option<Share>> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {SHARE_COLUMNS} FROM shared_channels
                 WHERE instance_id = ?1 AND remote_channel_id = ?2 AND role = 'guest'"
            ),
            params![instance_id, remote_channel_id],
            share_row,
        )
        .optional()?)
}

pub fn save_share(conn: &Connection, share: &Share, now: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO shared_channels (channel_id, instance_id, role, remote_channel_id, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (channel_id, instance_id) DO UPDATE SET role = excluded.role,
             remote_channel_id = COALESCE(excluded.remote_channel_id, shared_channels.remote_channel_id),
             status = excluded.status",
        params![
            share.channel_id,
            share.instance_id,
            share.role.key(),
            share.remote_channel_id,
            share.status.key(),
            now
        ],
    )?;
    Ok(())
}

/// Whether `channel_id` is shared with any server now.
pub fn is_shared(conn: &Connection, channel_id: i64) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM shared_channels WHERE channel_id = ?1 AND status = 'active')",
        [channel_id],
        |row| row.get(0),
    )?)
}

/// The servers a channel is shared with, for its settings: the share and
/// the other server.
pub fn shared_with(conn: &Connection, channel_id: i64) -> AppResult<Vec<(Share, Instance)>> {
    let mut found = Vec::new();
    for share in shares_of(conn, channel_id)? {
        if let Some(instance) = instance(conn, share.instance_id)? {
            found.push((share, instance));
        }
    }
    Ok(found)
}

/// A channel another server offered to share.
#[derive(Debug, Clone)]
pub struct Offer {
    pub id: i64,
    pub instance_id: i64,
    pub remote_channel_id: i64,
    pub name: String,
    pub topic: String,
    pub private: bool,
}

pub fn save_offer(conn: &Connection, offer: &Offer, now: i64) -> AppResult<()> {
    conn.execute(
        "INSERT INTO channel_offers (instance_id, remote_channel_id, name, topic, private, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (instance_id, remote_channel_id) DO UPDATE SET name = excluded.name,
             topic = excluded.topic, private = excluded.private",
        params![
            offer.instance_id,
            offer.remote_channel_id,
            offer.name,
            offer.topic,
            offer.private,
            now
        ],
    )?;
    Ok(())
}

pub fn offers(conn: &Connection) -> AppResult<Vec<Offer>> {
    let mut statement = conn.prepare(
        "SELECT id, instance_id, remote_channel_id, name, topic, private FROM channel_offers ORDER BY id",
    )?;
    let offers = statement
        .query_map([], |row| {
            Ok(Offer {
                id: row.get(0)?,
                instance_id: row.get(1)?,
                remote_channel_id: row.get(2)?,
                name: row.get(3)?,
                topic: row.get(4)?,
                private: row.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    Ok(offers)
}

/// Removes an offer and returns it.
pub fn take_offer(conn: &Connection, id: i64) -> AppResult<Option<Offer>> {
    let offer = offers(conn)?.into_iter().find(|offer| offer.id == id);
    conn.execute("DELETE FROM channel_offers WHERE id = ?1", [id])?;
    Ok(offer)
}

pub fn drop_offer(conn: &Connection, instance_id: i64, remote_channel_id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM channel_offers WHERE instance_id = ?1 AND remote_channel_id = ?2",
        params![instance_id, remote_channel_id],
    )?;
    Ok(())
}

// Outbox

/// Queues `event` for `instance_id`.
pub fn enqueue(
    conn: &Connection,
    instance_id: i64,
    event: &super::events::Event,
    now: i64,
) -> AppResult<()> {
    let json = serde_json::to_string(event).map_err(crate::error::AppError::internal)?;
    conn.execute(
        "INSERT INTO federation_outbox (instance_id, event, next_at, created_at) VALUES (?1, ?2, ?3, ?3)",
        params![instance_id, json, now],
    )?;
    Ok(())
}

/// Servers with events due by `now`.
pub fn due(conn: &Connection, now: i64) -> AppResult<Vec<i64>> {
    let mut statement = conn.prepare(
        "SELECT DISTINCT instance_id FROM federation_outbox o
         WHERE (SELECT MIN(next_at) FROM federation_outbox WHERE instance_id = o.instance_id) <= ?1",
    )?;
    let due = statement
        .query_map([now], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    Ok(due)
}

/// The oldest `limit` events waiting for `instance_id`: ids and JSON.
pub fn waiting(conn: &Connection, instance_id: i64, limit: i64) -> AppResult<Vec<(i64, String)>> {
    let mut statement = conn.prepare(
        "SELECT id, event FROM federation_outbox WHERE instance_id = ?1 ORDER BY id LIMIT ?2",
    )?;
    let waiting = statement
        .query_map(params![instance_id, limit], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<_, _>>()?;
    Ok(waiting)
}

pub fn delivered(conn: &Connection, ids: &[i64]) -> AppResult<()> {
    let ids = serde_json::to_string(ids).map_err(crate::error::AppError::internal)?;
    conn.execute(
        "DELETE FROM federation_outbox WHERE id IN (SELECT value FROM json_each(?1))",
        [ids],
    )?;
    Ok(())
}

/// Backs off from a server that didn't take its events: 5 seconds, then
/// doubling up to an hour.
pub fn failed(conn: &Connection, instance_id: i64, error: &str, now: i64) -> AppResult<()> {
    let attempts: i64 = conn.query_row(
        "SELECT COALESCE(MAX(attempts), 0) FROM federation_outbox WHERE instance_id = ?1",
        [instance_id],
        |row| row.get(0),
    )?;
    let delay_s = 5_i64
        .checked_shl(u32::try_from(attempts.clamp(0, 10)).unwrap_or(10))
        .unwrap_or(3600)
        .min(3600);
    conn.execute(
        "UPDATE federation_outbox SET attempts = attempts + 1, next_at = ?1, last_error = ?2
         WHERE instance_id = ?3",
        params![
            now.saturating_add(delay_s.saturating_mul(1000)),
            error,
            instance_id
        ],
    )?;
    Ok(())
}

/// How many events wait for `instance_id`, and why the last try failed.
pub fn backlog(conn: &Connection, instance_id: i64) -> AppResult<(i64, Option<String>)> {
    Ok(conn.query_row(
        "SELECT COUNT(*), MAX(last_error) FROM federation_outbox WHERE instance_id = ?1",
        [instance_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

// People from other servers

/// A person as other servers see them: from this server, or relayed.
pub fn person(
    conn: &Connection,
    own: &Own<'_>,
    user_id: i64,
) -> AppResult<Option<super::events::Person>> {
    let found: Option<(String, String, Option<i64>, Option<i64>)> = conn
        .query_row(
            "SELECT username, display_name, instance_id, remote_id FROM users WHERE id = ?1",
            [user_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    let Some((username, display_name, instance_id, remote_id)) = found else {
        return Ok(None);
    };
    Ok(match (instance_id, remote_id) {
        (Some(instance_id), Some(remote_id)) => {
            instance(conn, instance_id)?.map(|instance| super::events::Person {
                username: username
                    .split_once('@')
                    .map_or(username.as_str(), |(name, _)| name)
                    .to_owned(),
                server: instance.handle,
                url: instance.url,
                id: remote_id,
                display_name,
            })
        }
        _ => Some(super::events::Person {
            server: own.handle.to_owned(),
            url: own.url.to_owned(),
            id: user_id,
            username,
            display_name,
        }),
    })
}

/// This server, as events name it.
#[derive(Debug, Clone, Copy)]
pub struct Own<'a> {
    pub handle: &'a str,
    pub url: &'a str,
}

/// The server a person is on, remembering it as relayed if new.
fn instance_for(conn: &Connection, person: &super::events::Person, now: i64) -> AppResult<i64> {
    if let Some(instance) = instance_by_handle(conn, &person.server)? {
        return Ok(instance.id);
    }
    save_instance(
        conn,
        &Seen {
            url: person.url.trim_end_matches('/'),
            handle: &person.server,
            name: &person.server,
            public_key: None,
            status: Status::Relayed,
            note: "",
        },
        now,
    )
}

/// The account here for `person`: ours if they're from here, otherwise the
/// one standing in for them, made or updated now. `None` for people who
/// don't exist here or are deactivated.
pub fn account(
    conn: &Connection,
    own: &Own<'_>,
    person: &super::events::Person,
    now: i64,
) -> AppResult<Option<i64>> {
    if person.server.eq_ignore_ascii_case(own.handle) {
        return Ok(conn
            .query_row(
                "SELECT id FROM users WHERE id = ?1 AND instance_id IS NULL AND deactivated_at IS NULL",
                [person.id],
                |row| row.get(0),
            )
            .optional()?);
    }
    let instance_id = instance_for(conn, person, now)?;
    let username = format!("{}@{}", person.username, person.server).to_lowercase();
    let display_name: String = person.display_name.chars().take(64).collect();
    let existing: Option<(i64, bool)> = conn
        .query_row(
            "SELECT id, deactivated_at IS NOT NULL FROM users WHERE instance_id = ?1 AND remote_id = ?2",
            params![instance_id, person.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((id, deactivated)) = existing {
        if deactivated {
            return Ok(None);
        }
        conn.execute(
            "UPDATE users SET display_name = ?1, username = ?2 WHERE id = ?3",
            params![display_name, username, id],
        )?;
        return Ok(Some(id));
    }
    // An account that can't sign in: no password, no email, no passkeys.
    conn.execute(
        "INSERT INTO users (username, display_name, password_hash, created_at, instance_id, remote_id, trust_level)
         VALUES (?1, ?2, '', ?3, ?4, ?5, 1)",
        params![username, display_name, now, instance_id, person.id],
    )?;
    Ok(Some(conn.last_insert_rowid()))
}

/// The server a person stands in for, if they're from elsewhere.
pub fn remote_server(conn: &Connection, user_id: i64) -> AppResult<Option<Instance>> {
    let instance_id: Option<i64> = conn
        .query_row(
            "SELECT instance_id FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    instance_id.map_or(Ok(None), |id| instance(conn, id))
}
