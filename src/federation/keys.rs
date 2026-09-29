//! This server's key, and signing and checking requests between servers.
//!
//! Every request one Sideporch sends another carries these headers:
//!
//! - `Sideporch-Server`: the sender's URL, which names its pinned key;
//! - `Sideporch-Date`: when it was signed, in milliseconds;
//! - `Sideporch-Nonce`: random, never used twice;
//! - `Sideporch-Signature`: Ed25519 over the method, the path, the
//!   receiver's URL, the sender's URL, the date, the nonce and the body's
//!   SHA-256.
//!
//! Naming the receiver keeps a signed request from being replayed to
//! another server; the date and nonce keep it from being replayed at all.

use base64ct::{Base64, Encoding as _};
use ring::{
    rand::SystemRandom,
    signature::{ED25519, Ed25519KeyPair, KeyPair as _, UnparsedPublicKey},
};
use sha2::{Digest as _, Sha256};

use crate::error::{AppError, AppResult};

pub const SERVER: &str = "sideporch-server";
pub const DATE: &str = "sideporch-date";
pub const NONCE: &str = "sideporch-nonce";
pub const SIGNATURE: &str = "sideporch-signature";
/// How far a request's date may be from the receiver's clock.
pub const CLOCK_SKEW_MS: i64 = 5 * 60 * 1000;

/// This server's signing key.
pub struct Identity {
    pair: Ed25519KeyPair,
}

impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("public_key", &Base64::encode_string(self.public_key()))
            .finish_non_exhaustive()
    }
}

impl Identity {
    /// A new key, and its PKCS#8 form to keep.
    pub fn generate() -> AppResult<(Self, Vec<u8>)> {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
            .map_err(|_| AppError::internal("could not make a server key"))?;
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| AppError::internal("could not read the new server key"))?;
        Ok((Self { pair }, pkcs8.as_ref().to_vec()))
    }

    pub fn from_pkcs8(pkcs8: &[u8]) -> AppResult<Self> {
        Ed25519KeyPair::from_pkcs8(pkcs8)
            .map(|pair| Self { pair })
            .map_err(|_| AppError::internal("the stored server key is damaged"))
    }

    pub fn public_key(&self) -> &[u8] {
        self.pair.public_key().as_ref()
    }

    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.pair.sign(data).as_ref().to_vec()
    }

    /// The headers for a request from `sender` (our URL) to `target` (the
    /// receiver's URL), for `path` with its query.
    pub fn sign_request(
        &self,
        method: &str,
        path: &str,
        target: &str,
        sender: &str,
        body: &[u8],
        date: i64,
    ) -> AppResult<Vec<(&'static str, String)>> {
        let nonce = crate::auth::random_token()?;
        let signature = self.sign(&signing_input(
            method, path, target, sender, date, &nonce, body,
        ));
        Ok(vec![
            (SERVER, sender.to_owned()),
            (DATE, date.to_string()),
            (NONCE, nonce),
            (SIGNATURE, Base64::encode_string(&signature)),
        ])
    }
}

/// What a request's signature covers.
fn signing_input(
    method: &str,
    path: &str,
    target: &str,
    sender: &str,
    date: i64,
    nonce: &str,
    body: &[u8],
) -> Vec<u8> {
    let digest = Sha256::digest(body);
    let hash: String = digest.iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        // Writing to a String cannot fail.
        let _ = write!(hex, "{byte:02x}");
        hex
    });
    format!(
        "sideporch-request-v1\n{}\n{path}\n{target}\n{sender}\n{date}\n{nonce}\n{hash}",
        method.to_ascii_uppercase()
    )
    .into_bytes()
}

/// The signature headers of a request, before checking them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    pub server: String,
    pub date: i64,
    pub nonce: String,
    signature: Vec<u8>,
}

impl Signed {
    /// Reads the headers; `header` looks one up by its lowercase name.
    pub fn read<'a>(header: impl Fn(&str) -> Option<&'a str>) -> Result<Self, String> {
        let missing = |name: &str| format!("the request has no {name} header");
        let server = header(SERVER)
            .ok_or_else(|| missing(SERVER))?
            .trim_end_matches('/')
            .to_owned();
        let date = header(DATE)
            .ok_or_else(|| missing(DATE))?
            .parse()
            .map_err(|_| "the request's date isn't a number".to_owned())?;
        let nonce = header(NONCE).ok_or_else(|| missing(NONCE))?.to_owned();
        if nonce.is_empty() || nonce.len() > 128 {
            return Err("the request's nonce isn't valid".to_owned());
        }
        let signature = Base64::decode_vec(header(SIGNATURE).ok_or_else(|| missing(SIGNATURE))?)
            .map_err(|_| "the request's signature isn't valid".to_owned())?;
        Ok(Self {
            server,
            date,
            nonce,
            signature,
        })
    }

    /// Checks the signature with `public_key`, and that the request was
    /// meant for `target` (our URL) and signed recently.
    pub fn check(
        &self,
        public_key: &[u8],
        method: &str,
        path: &str,
        target: &str,
        body: &[u8],
        now: i64,
    ) -> Result<(), String> {
        if now.abs_diff(self.date) > CLOCK_SKEW_MS.unsigned_abs() {
            return Err(
                "the request's date is too far from now; check both servers' clocks".to_owned(),
            );
        }
        let input = signing_input(
            method,
            path,
            target,
            &self.server,
            self.date,
            &self.nonce,
            body,
        );
        UnparsedPublicKey::new(&ED25519, public_key)
            .verify(&input, &self.signature)
            .map_err(|_| "the request's signature doesn't match the server's key".to_owned())
    }
}

/// A key's fingerprint, to compare over the phone: 8 groups of 4 hex digits.
pub fn fingerprint(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .chunks(2)
        .map(<[String]>::concat)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers<'a>(list: &'a [(&'static str, String)]) -> impl Fn(&str) -> Option<&'a str> {
        move |name| {
            list.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.as_str())
        }
    }

    #[test]
    fn signed_requests_check_out_only_as_sent() {
        let (key, _) = Identity::generate().unwrap();
        let now = 1_000_000;
        let sent = key
            .sign_request(
                "post",
                "/federation/inbox",
                "https://b.example",
                "https://a.example",
                b"{}",
                now,
            )
            .unwrap();
        let signed = Signed::read(headers(&sent)).unwrap();
        assert_eq!(signed.server, "https://a.example");
        let public = key.public_key();
        let check = |method, path, target, body: &[u8], at| {
            signed.check(public, method, path, target, body, at)
        };
        assert_eq!(
            check("POST", "/federation/inbox", "https://b.example", b"{}", now),
            Ok(())
        );
        assert!(
            check(
                "POST",
                "/federation/inbox",
                "https://b.example",
                b"{ }",
                now
            )
            .is_err(),
            "body"
        );
        assert!(
            check("POST", "/federation/other", "https://b.example", b"{}", now).is_err(),
            "path"
        );
        assert!(
            check("POST", "/federation/inbox", "https://c.example", b"{}", now).is_err(),
            "receiver"
        );
        assert!(
            check("GET", "/federation/inbox", "https://b.example", b"{}", now).is_err(),
            "method"
        );
        assert!(
            check(
                "POST",
                "/federation/inbox",
                "https://b.example",
                b"{}",
                now + CLOCK_SKEW_MS + 1
            )
            .is_err(),
            "too late"
        );
        let (other, _) = Identity::generate().unwrap();
        assert!(
            signed
                .check(
                    other.public_key(),
                    "POST",
                    "/federation/inbox",
                    "https://b.example",
                    b"{}",
                    now
                )
                .is_err()
        );
    }

    #[test]
    fn keys_survive_being_stored() {
        let (key, pkcs8) = Identity::generate().unwrap();
        let again = Identity::from_pkcs8(&pkcs8).unwrap();
        assert_eq!(key.public_key(), again.public_key());
        assert_eq!(fingerprint(key.public_key()).split(' ').count(), 8);
        assert!(Signed::read(|_| None).is_err());
    }
}
