//! How people prove who they are: the sign-in policy, and what each person
//! set up besides their password (passkeys, an authenticator app, recovery
//! codes, a confirmed email address).

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::{
    error::{AppError, AppResult},
    passkeys, store,
};

/// What admins require of everyone's sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Requirement {
    /// A password is enough; a second step is up to each person.
    #[default]
    None,
    /// Admins must have a passkey or an authenticator app.
    AdminsSecondStep,
    /// Everyone must.
    EveryoneSecondStep,
    /// Everyone signs in with a passkey; a password only works until they
    /// have added one.
    Passkeys,
}

impl Requirement {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "admins" => Some(Self::AdminsSecondStep),
            "everyone" => Some(Self::EveryoneSecondStep),
            "passkeys" => Some(Self::Passkeys),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::AdminsSecondStep => "admins",
            Self::EveryoneSecondStep => "everyone",
            Self::Passkeys => "passkeys",
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Policy {
    pub require: Requirement,
    /// People may sign in with a link sent to their confirmed address.
    pub email_links: bool,
}

impl Policy {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            require: store::setting(conn, "auth.require")?
                .as_deref()
                .and_then(Requirement::parse)
                .unwrap_or_default(),
            email_links: store::setting(conn, "auth.email_links")?.as_deref() == Some("true"),
        })
    }

    pub fn save(self, conn: &Connection) -> AppResult<()> {
        store::set_setting(conn, "auth.require", self.require.key())?;
        store::set_setting(
            conn,
            "auth.email_links",
            if self.email_links { "true" } else { "false" },
        )
    }

    /// Whether someone still has to set up more before using Sideporch.
    pub const fn needs_more(self, is_admin: bool, factors: Factors) -> bool {
        match self.require {
            Requirement::None => false,
            Requirement::AdminsSecondStep => is_admin && !factors.second_step(),
            Requirement::EveryoneSecondStep => !factors.second_step(),
            Requirement::Passkeys => factors.passkeys == 0,
        }
    }
}

/// What someone set up besides their password.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Factors {
    pub passkeys: i64,
    pub totp: bool,
    pub recovery_codes: i64,
}

impl Factors {
    /// Whether signing in with a password takes a second step.
    pub const fn second_step(self) -> bool {
        self.passkeys > 0 || self.totp
    }
}

pub fn factors(conn: &Connection, user_id: i64) -> AppResult<Factors> {
    Ok(conn.query_row(
        "SELECT (SELECT COUNT(*) FROM passkeys WHERE user_id = ?1),
                (SELECT totp_secret IS NOT NULL FROM users WHERE id = ?1),
                (SELECT COUNT(*) FROM recovery_codes WHERE user_id = ?1 AND used_at IS NULL)",
        [user_id],
        |row| {
            Ok(Factors {
                passkeys: row.get(0)?,
                totp: row.get::<_, Option<bool>>(1)?.unwrap_or(false),
                recovery_codes: row.get(2)?,
            })
        },
    )?)
}

// Passkeys

#[derive(Debug, Clone)]
pub struct PasskeyInfo {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

pub fn passkeys(conn: &Connection, user_id: i64) -> AppResult<Vec<PasskeyInfo>> {
    let mut statement = conn.prepare(
        "SELECT id, name, created_at, last_used_at FROM passkeys WHERE user_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map([user_id], |row| {
        Ok(PasskeyInfo {
            id: row.get(0)?,
            name: row.get(1)?,
            created_at: row.get(2)?,
            last_used_at: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// The credential ids of someone's passkeys, for the browser.
pub fn credential_ids(conn: &Connection, user_id: i64) -> AppResult<Vec<(Vec<u8>, Vec<String>)>> {
    let mut statement =
        conn.prepare("SELECT credential_id, transports FROM passkeys WHERE user_id = ?1")?;
    let rows = statement.query_map([user_id], |row| {
        let transports: String = row.get(1)?;
        Ok((
            row.get(0)?,
            transports
                .split(',')
                .filter(|transport| !transport.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn add_passkey(
    conn: &Connection,
    user_id: i64,
    passkey: &passkeys::NewPasskey,
    name: &str,
    now: i64,
) -> AppResult<()> {
    let taken: bool = conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM passkeys WHERE credential_id = ?1)",
        [&passkey.credential_id],
        |row| row.get(0),
    )?;
    if taken {
        return Err(AppError::bad_request("That passkey is already added."));
    }
    conn.execute(
        "INSERT INTO passkeys (user_id, credential_id, public_key, algorithm, sign_count, transports, name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            user_id,
            passkey.credential_id,
            passkey.public_key,
            passkey.algorithm,
            passkey.sign_count,
            passkey.transports.join(","),
            name,
            now
        ],
    )?;
    Ok(())
}

pub fn passkey_by_credential(
    conn: &Connection,
    credential_id: &[u8],
) -> AppResult<Option<passkeys::Stored>> {
    Ok(conn
        .query_row(
            "SELECT p.id, p.user_id, p.public_key, p.algorithm, p.sign_count FROM passkeys p
             JOIN users u ON u.id = p.user_id
             WHERE p.credential_id = ?1 AND u.deactivated_at IS NULL",
            [credential_id],
            |row| {
                Ok(passkeys::Stored {
                    id: row.get(0)?,
                    user_id: row.get(1)?,
                    public_key: row.get(2)?,
                    algorithm: row.get(3)?,
                    sign_count: row.get(4)?,
                })
            },
        )
        .optional()?)
}

pub fn passkey_used(conn: &Connection, id: i64, sign_count: u32, now: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE passkeys SET sign_count = ?1, last_used_at = ?2 WHERE id = ?3",
        params![sign_count, now, id],
    )?;
    Ok(())
}

pub fn delete_passkey(conn: &Connection, user_id: i64, id: i64) -> AppResult<()> {
    conn.execute(
        "DELETE FROM passkeys WHERE id = ?1 AND user_id = ?2",
        params![id, user_id],
    )?;
    Ok(())
}

/// The opaque id the browser keeps with someone's passkeys; made once.
pub fn user_handle(conn: &Connection, user_id: i64) -> AppResult<Vec<u8>> {
    let existing: Option<Vec<u8>> = conn.query_row(
        "SELECT webauthn_handle FROM users WHERE id = ?1",
        [user_id],
        |row| row.get(0),
    )?;
    if let Some(handle) = existing {
        return Ok(handle);
    }
    let mut handle = vec![0_u8; 32];
    getrandom::fill(&mut handle).map_err(AppError::internal)?;
    conn.execute(
        "UPDATE users SET webauthn_handle = ?1 WHERE id = ?2",
        params![handle, user_id],
    )?;
    Ok(handle)
}

// Authenticator apps

/// The sealed secret and the last step used, if set up.
pub fn totp(conn: &Connection, user_id: i64) -> AppResult<Option<(String, u64)>> {
    Ok(conn
        .query_row(
            "SELECT totp_secret, totp_last_step FROM users WHERE id = ?1 AND totp_secret IS NOT NULL",
            [user_id],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?
        .map(|(secret, step)| (secret, u64::try_from(step).unwrap_or(0))))
}

pub fn set_totp(conn: &Connection, user_id: i64, sealed: Option<&str>, step: u64) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET totp_secret = ?1, totp_last_step = ?2 WHERE id = ?3",
        params![sealed, i64::try_from(step).unwrap_or(0), user_id],
    )?;
    Ok(())
}

/// Records the step of a code just used, so it can't be used again.
pub fn totp_used(conn: &Connection, user_id: i64, step: u64) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET totp_last_step = ?1 WHERE id = ?2",
        params![i64::try_from(step).unwrap_or(0), user_id],
    )?;
    Ok(())
}

// Recovery codes

pub fn replace_recovery_codes(conn: &Connection, user_id: i64, codes: &[String]) -> AppResult<()> {
    conn.execute("DELETE FROM recovery_codes WHERE user_id = ?1", [user_id])?;
    for code in codes {
        conn.execute(
            "INSERT INTO recovery_codes (user_id, code_hash) VALUES (?1, ?2)",
            params![
                user_id,
                crate::auth::hash_token(&crate::totp::normalize_recovery(code))
            ],
        )?;
    }
    Ok(())
}

/// Uses up a recovery code, if it is one of theirs and still unused.
pub fn use_recovery_code(conn: &Connection, user_id: i64, code: &str, now: i64) -> AppResult<bool> {
    let hash = crate::auth::hash_token(&crate::totp::normalize_recovery(code));
    Ok(conn.execute(
        "UPDATE recovery_codes SET used_at = ?1 WHERE user_id = ?2 AND code_hash = ?3 AND used_at IS NULL",
        params![now, user_id, hash],
    )? > 0)
}

/// Removes everything but the password, when someone lost their devices.
pub fn reset_factors(conn: &Connection, user_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM passkeys WHERE user_id = ?1", [user_id])?;
    conn.execute("DELETE FROM recovery_codes WHERE user_id = ?1", [user_id])?;
    set_totp(conn, user_id, None, 0)
}

// Signing in in steps

/// Remembers someone who passed the first step, for the second.
pub fn start_pending(
    conn: &Connection,
    token_hash: &[u8],
    user_id: i64,
    now: i64,
) -> AppResult<()> {
    conn.execute("DELETE FROM pending_logins WHERE expires_at < ?1", [now])?;
    conn.execute(
        "INSERT INTO pending_logins (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![token_hash, user_id, now, now.saturating_add(10 * 60 * 1000)],
    )?;
    Ok(())
}

/// Who a pending sign-in is for. Each wrong attempt counts; after five
/// the pending sign-in ends.
pub fn pending(conn: &Connection, token_hash: &[u8], now: i64) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT user_id FROM pending_logins WHERE token_hash = ?1 AND expires_at > ?2 AND attempts < 5",
            params![token_hash, now],
            |row| row.get(0),
        )
        .optional()?)
}

pub fn failed_attempt(conn: &Connection, token_hash: &[u8]) -> AppResult<()> {
    conn.execute(
        "UPDATE pending_logins SET attempts = attempts + 1 WHERE token_hash = ?1",
        [token_hash],
    )?;
    Ok(())
}

pub fn end_pending(conn: &Connection, token_hash: &[u8]) -> AppResult<()> {
    conn.execute(
        "DELETE FROM pending_logins WHERE token_hash = ?1",
        [token_hash],
    )?;
    Ok(())
}

// Email

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkPurpose {
    SignIn,
    ConfirmEmail,
    ResetPassword,
}

impl LinkPurpose {
    const fn key(self) -> &'static str {
        match self {
            Self::SignIn => "login",
            Self::ConfirmEmail => "confirm",
            Self::ResetPassword => "reset",
        }
    }
}

/// Stores a link token for someone; confirming an address carries it.
pub fn create_link(
    conn: &Connection,
    token_hash: &[u8],
    user_id: i64,
    purpose: LinkPurpose,
    email: &str,
    now: i64,
    valid_ms: i64,
) -> AppResult<()> {
    conn.execute("DELETE FROM login_links WHERE expires_at < ?1", [now])?;
    conn.execute(
        "INSERT INTO login_links (token_hash, user_id, purpose, email, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            token_hash,
            user_id,
            purpose.key(),
            email,
            now,
            now.saturating_add(valid_ms)
        ],
    )?;
    Ok(())
}

/// Whether a link token is valid, without using it up.
pub fn link_valid(
    conn: &Connection,
    token_hash: &[u8],
    purpose: LinkPurpose,
    now: i64,
) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM login_links WHERE token_hash = ?1 AND purpose = ?2 AND expires_at > ?3)",
        params![token_hash, purpose.key(), now],
        |row| row.get(0),
    )?)
}

/// Uses up a link token and returns whose it was and its address.
pub fn take_link(
    conn: &Connection,
    token_hash: &[u8],
    purpose: LinkPurpose,
    now: i64,
) -> AppResult<Option<(i64, String)>> {
    let found: Option<(i64, String)> = conn
        .query_row(
            "SELECT user_id, email FROM login_links WHERE token_hash = ?1 AND purpose = ?2 AND expires_at > ?3",
            params![token_hash, purpose.key(), now],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    conn.execute(
        "DELETE FROM login_links WHERE token_hash = ?1",
        [token_hash],
    )?;
    Ok(found)
}

/// Someone's confirmed address.
pub fn email(conn: &Connection, user_id: i64) -> AppResult<Option<String>> {
    Ok(
        conn.query_row("SELECT email FROM users WHERE id = ?1", [user_id], |row| {
            row.get(0)
        })?,
    )
}

pub fn set_email(conn: &Connection, user_id: i64, email: Option<&str>) -> AppResult<()> {
    if let Some(email) = email {
        conn.execute(
            "UPDATE users SET email = NULL WHERE email = ?1 AND id != ?2",
            params![email, user_id],
        )?;
    }
    conn.execute(
        "UPDATE users SET email = ?1 WHERE id = ?2",
        params![email, user_id],
    )?;
    Ok(())
}

/// The active account with this confirmed address.
pub fn user_by_email(conn: &Connection, email: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM users WHERE email = ?1 AND deactivated_at IS NULL",
            [email.to_lowercase()],
            |row| row.get(0),
        )
        .optional()?)
}

/// How many links someone was sent in the last hour, to stop floods.
pub fn recent_links(conn: &Connection, user_id: i64, now: i64) -> AppResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM login_links WHERE user_id = ?1 AND created_at > ?2",
        params![user_id, now.saturating_sub(60 * 60 * 1000)],
        |row| row.get(0),
    )?)
}
