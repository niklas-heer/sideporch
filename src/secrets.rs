//! Secrets for automations, such as API tokens.
//!
//! Admins store secrets in the database, encrypted with AES-256-GCM. The
//! key never enters the database: it comes from `SIDEPORCH_SECRET_KEY` or
//! from `secret.key` in the data directory, which is created on first start
//! and readable only by the server's user. A database backup alone does
//! not reveal secrets; back up the key with it.
//!
//! Environment variables named `SIDEPORCH_SECRET_<NAME>` are secrets too,
//! and take precedence over stored ones with the same name. Scripts read
//! them with `sideporch.secret("NAME")`; nobody can read them back in the
//! interface.

use std::{collections::BTreeMap, path::Path};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64ct::{Base64UrlUnpadded, Encoding as _};
use rusqlite::{Connection, params};
use sha2::{Digest as _, Sha256};

use crate::error::{AppError, AppResult};

const ENV_PREFIX: &str = "SIDEPORCH_SECRET_";
const KEY_ENV: &str = "SIDEPORCH_SECRET_KEY";
const KEY_FILE: &str = "secret.key";
const MAX_VALUE_BYTES: usize = 64 * 1024;

/// Encrypts and decrypts stored secrets.
pub struct Vault {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Vault")
    }
}

fn random<const N: usize>() -> AppResult<[u8; N]> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(AppError::internal)?;
    Ok(bytes)
}

impl Vault {
    /// Opens the vault with the key from the environment, or from the data
    /// directory's key file, creating that file on first use.
    pub fn open(data_dir: &Path) -> AppResult<Self> {
        let key: [u8; 32] = if let Ok(passphrase) = std::env::var(KEY_ENV) {
            // Any string works; it is stretched to a key by hashing.
            Sha256::digest(format!("sideporch secrets\n{passphrase}")).into()
        } else {
            let path = data_dir.join(KEY_FILE);
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let bytes = Base64UrlUnpadded::decode_vec(text.trim())
                        .map_err(|_| AppError::internal("secret.key is not valid"))?;
                    bytes
                        .try_into()
                        .map_err(|_| AppError::internal("secret.key has the wrong length"))?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let key = random::<32>()?;
                    crate::write_private(&path, &Base64UrlUnpadded::encode_string(&key))
                        .map_err(AppError::internal)?;
                    key
                }
                Err(error) => return Err(AppError::internal(error)),
            }
        };
        Ok(Self {
            cipher: Aes256Gcm::new(&key.into()),
        })
    }

    /// Encrypts `value`, bound to `name` so it cannot be swapped with
    /// another secret. Returns the nonce and the ciphertext.
    pub fn seal(&self, name: &str, value: &str) -> AppResult<(Vec<u8>, Vec<u8>)> {
        let nonce = random::<12>()?;
        let sealed = self
            .cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: value.as_bytes(),
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| AppError::internal("could not encrypt a secret"))?;
        Ok((nonce.to_vec(), sealed))
    }

    pub fn open_sealed(&self, name: &str, nonce: &[u8], sealed: &[u8]) -> AppResult<String> {
        let nonce: [u8; 12] = nonce
            .try_into()
            .map_err(|_| AppError::internal("a stored secret is damaged"))?;
        let plain = self
            .cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: sealed,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| {
                AppError::internal(format!(
                    "could not decrypt the secret {name}; was secret.key or SIDEPORCH_SECRET_KEY changed?"
                ))
            })?;
        String::from_utf8(plain).map_err(AppError::internal)
    }

    /// Seals a value into one text, for settings such as the AI key.
    pub fn seal_text(&self, name: &str, value: &str) -> AppResult<String> {
        let (nonce, sealed) = self.seal(name, value)?;
        Ok(format!(
            "{}.{}",
            Base64UrlUnpadded::encode_string(&nonce),
            Base64UrlUnpadded::encode_string(&sealed)
        ))
    }

    pub fn open_text(&self, name: &str, text: &str) -> AppResult<String> {
        let (nonce, sealed) = text
            .split_once('.')
            .ok_or_else(|| AppError::internal("a stored secret is damaged"))?;
        let decode = |part: &str| {
            Base64UrlUnpadded::decode_vec(part)
                .map_err(|_| AppError::internal("a stored secret is damaged"))
        };
        self.open_sealed(name, &decode(nonce)?, &decode(sealed)?)
    }
}

/// Whether `name` can name a secret: `API_TOKEN`, `github_token`, …
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Secrets from `SIDEPORCH_SECRET_<NAME>` environment variables.
pub fn from_environment() -> BTreeMap<String, String> {
    std::env::vars()
        .filter_map(|(key, value)| {
            let name = key.strip_prefix(ENV_PREFIX)?;
            (name != "KEY" && valid_name(name)).then(|| (name.to_owned(), value))
        })
        .collect()
}

/// Where a secret comes from, for the settings page.
#[derive(Debug, Clone)]
pub struct SecretInfo {
    pub name: String,
    pub stored: bool,
    pub from_environment: bool,
    pub updated_at: Option<i64>,
    pub updated_by: Option<String>,
}

pub fn list(conn: &Connection) -> AppResult<Vec<SecretInfo>> {
    let mut statement = conn.prepare(
        "SELECT s.name, s.updated_at, u.display_name FROM secrets s
         LEFT JOIN users u ON u.id = s.updated_by ORDER BY s.name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(SecretInfo {
            name: row.get(0)?,
            stored: true,
            from_environment: false,
            updated_at: row.get(1)?,
            updated_by: row.get(2)?,
        })
    })?;
    let mut secrets: BTreeMap<String, SecretInfo> = BTreeMap::new();
    for row in rows {
        let info = row?;
        secrets.insert(info.name.clone(), info);
    }
    for name in from_environment().into_keys() {
        secrets
            .entry(name.clone())
            .or_insert(SecretInfo {
                name,
                stored: false,
                from_environment: false,
                updated_at: None,
                updated_by: None,
            })
            .from_environment = true;
    }
    Ok(secrets.into_values().collect())
}

/// Every secret's value, environment variables winning over stored ones.
pub fn all(conn: &Connection, vault: &Vault) -> AppResult<BTreeMap<String, String>> {
    let mut statement = conn.prepare("SELECT name, nonce, value FROM secrets")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    let mut secrets = BTreeMap::new();
    for row in rows {
        let (name, nonce, sealed) = row?;
        match vault.open_sealed(&name, &nonce, &sealed) {
            Ok(value) => {
                secrets.insert(name, value);
            }
            Err(error) => tracing::warn!(%error, "skipping a secret"),
        }
    }
    secrets.extend(from_environment());
    Ok(secrets)
}

pub fn set(
    conn: &Connection,
    vault: &Vault,
    name: &str,
    value: &str,
    user_id: i64,
    now: i64,
) -> AppResult<()> {
    if !valid_name(name) {
        return Err(AppError::bad_request(
            "Secret names start with a letter and use letters, digits and underscores, up to 64.",
        ));
    }
    if value.is_empty() || value.len() > MAX_VALUE_BYTES {
        return Err(AppError::bad_request(
            "A secret needs a value of up to 64 kB.",
        ));
    }
    let (nonce, sealed) = vault.seal(name, value)?;
    conn.execute(
        "INSERT INTO secrets (name, nonce, value, updated_by, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (name) DO UPDATE SET nonce = excluded.nonce, value = excluded.value,
             updated_by = excluded.updated_by, updated_at = excluded.updated_at",
        params![name, nonce, sealed, user_id, now],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, name: &str) -> AppResult<bool> {
    Ok(conn.execute("DELETE FROM secrets WHERE name = ?1", [name])? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seals_and_opens_values_bound_to_their_name() {
        let dir = tempfile::tempdir().unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let (nonce, sealed) = vault.seal("TOKEN", "hunter2").unwrap();
        assert_eq!(
            vault.open_sealed("TOKEN", &nonce, &sealed).unwrap(),
            "hunter2"
        );
        assert!(vault.open_sealed("OTHER", &nonce, &sealed).is_err());
        let text = vault.seal_text("ai", "sk-123").unwrap();
        assert_eq!(vault.open_text("ai", &text).unwrap(), "sk-123");

        // The key file persists, so a reopened vault reads old secrets.
        let again = Vault::open(dir.path()).unwrap();
        assert_eq!(
            again.open_sealed("TOKEN", &nonce, &sealed).unwrap(),
            "hunter2"
        );
        let key = dir.path().join(KEY_FILE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn checks_names() {
        assert!(valid_name("GITHUB_TOKEN") && valid_name("api2"));
        assert!(!valid_name("2FA") && !valid_name("with-dash") && !valid_name(""));
    }
}
