//! Passkeys: the relying-party side of `WebAuthn`.
//!
//! The browser creates a key pair on the person's device and gives us the
//! public key, in SPKI form (`getPublicKey()`), so no CBOR needs parsing.
//! Signing in, the device signs our challenge. Verification follows
//! `WebAuthn` Level 2 §7 for attestation `none`: the client data names the
//! ceremony, our challenge and our origin; the authenticator data is for
//! our RP ID, the user was present and verified; the signature checks out
//! with the stored key. `ES256`, `EdDSA` and `RS256` keys are accepted,
//! verified with ring.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use base64ct::{Base64UrlUnpadded, Encoding as _};
use ring::signature;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::error::{AppError, AppResult};

/// COSE algorithm numbers we accept, in order of preference.
pub const ALGORITHMS: [i64; 3] = [-8, -7, -257];
const EDDSA: i64 = -8;
const ES256: i64 = -7;
const RS256: i64 = -257;
/// How long a started ceremony stays valid.
const CEREMONY_MS: i64 = 5 * 60 * 1000;

const FLAG_USER_PRESENT: u8 = 0x01;
const FLAG_USER_VERIFIED: u8 = 0x04;
const FLAG_ATTESTED: u8 = 0x40;

pub fn encode(bytes: &[u8]) -> String {
    Base64UrlUnpadded::encode_string(bytes)
}

pub fn decode(text: &str) -> AppResult<Vec<u8>> {
    Base64UrlUnpadded::decode_vec(text.trim_end_matches('='))
        .map_err(|_| AppError::bad_request("The passkey answer was damaged."))
}

/// What a started ceremony is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Purpose {
    /// Adding a passkey to this account.
    Register(i64),
    /// Signing in with any passkey.
    SignIn,
    /// The second step of signing in, for this account.
    Confirm(i64),
}

#[derive(Debug, Clone)]
struct Ceremony {
    challenge: Vec<u8>,
    purpose: Purpose,
    expires_at: i64,
}

/// Challenges handed out and not used yet. They live in memory: a restart
/// only means trying again.
#[derive(Clone, Default)]
pub struct Ceremonies(Arc<Mutex<HashMap<String, Ceremony>>>);

impl Ceremonies {
    /// Starts a ceremony and returns its id and challenge.
    pub fn start(&self, purpose: Purpose, now: i64) -> AppResult<(String, Vec<u8>)> {
        let id = crate::auth::random_token()?;
        let mut challenge = vec![0_u8; 32];
        getrandom::fill(&mut challenge).map_err(AppError::internal)?;
        let ceremony = Ceremony {
            challenge: challenge.clone(),
            purpose,
            expires_at: now.saturating_add(CEREMONY_MS),
        };
        let mut ceremonies = self
            .0
            .lock()
            .map_err(|_| AppError::internal("ceremonies lock poisoned"))?;
        ceremonies.retain(|_, ceremony| ceremony.expires_at > now);
        // A flood of started ceremonies must not grow memory without bound.
        if ceremonies.len() > 10_000 {
            return Err(AppError::bad_request(
                "Too many sign-ins at once. Try again in a minute.",
            ));
        }
        ceremonies.insert(id.clone(), ceremony);
        drop(ceremonies);
        Ok((id, challenge))
    }

    /// Ends a ceremony, returning its challenge if it was for `purpose`
    /// and hasn't expired. Each works once.
    pub fn finish(&self, id: &str, purpose: &Purpose, now: i64) -> AppResult<Vec<u8>> {
        let ceremony = self
            .0
            .lock()
            .map_err(|_| AppError::internal("ceremonies lock poisoned"))?
            .remove(id);
        match ceremony {
            Some(ceremony) if ceremony.expires_at > now && &ceremony.purpose == purpose => {
                Ok(ceremony.challenge)
            }
            _ => Err(AppError::bad_request(
                "That passkey request expired. Try again.",
            )),
        }
    }
}

/// Where the ceremony happens: our RP ID (the host name) and origin.
#[derive(Debug, Clone)]
pub struct Site {
    pub rp_id: String,
    pub origin: String,
}

impl Site {
    /// From the base URL people reach Sideporch at.
    pub fn from_base_url(base: &str) -> Self {
        let host = base
            .split_once("://")
            .map_or(base, |(_, rest)| rest)
            .split('/')
            .next()
            .unwrap_or_default();
        // Strip a port, but keep IPv6 brackets intact.
        let rp_id = if host.starts_with('[') {
            host.split(']')
                .next()
                .map_or(host, |inner| inner.trim_start_matches('['))
        } else {
            host.split(':').next().unwrap_or(host)
        };
        Self {
            rp_id: rp_id.to_owned(),
            origin: base.trim_end_matches('/').to_owned(),
        }
    }
}

#[derive(Deserialize)]
struct ClientData {
    #[serde(rename = "type")]
    kind: String,
    challenge: String,
    origin: String,
    #[serde(default, rename = "crossOrigin")]
    cross_origin: bool,
}

fn check_client_data(json: &[u8], kind: &str, challenge: &[u8], site: &Site) -> AppResult<()> {
    let data: ClientData = serde_json::from_slice(json)
        .map_err(|_| AppError::bad_request("The passkey answer was damaged."))?;
    if data.kind != kind || data.cross_origin {
        return Err(AppError::bad_request(
            "That passkey answer was for something else.",
        ));
    }
    let expected = encode(challenge);
    if !crate::auth::same_bytes(
        data.challenge.trim_end_matches('=').as_bytes(),
        expected.as_bytes(),
    ) {
        return Err(AppError::bad_request(
            "That passkey request expired. Try again.",
        ));
    }
    if data.origin != site.origin {
        return Err(AppError::bad_request(format!(
            "The passkey was used on {}, not {}. Open Sideporch at its usual address.",
            data.origin, site.origin
        )));
    }
    Ok(())
}

/// The fixed part of authenticator data.
struct AuthData<'a> {
    flags: u8,
    sign_count: u32,
    rest: &'a [u8],
}

fn parse_auth_data<'a>(data: &'a [u8], site: &Site) -> AppResult<AuthData<'a>> {
    let damaged = || AppError::bad_request("The passkey answer was damaged.");
    let rp_hash = data.get(..32).ok_or_else(damaged)?;
    if rp_hash != Sha256::digest(site.rp_id.as_bytes()).as_slice() {
        return Err(AppError::bad_request(
            "That passkey belongs to another site.",
        ));
    }
    let flags = *data.get(32).ok_or_else(damaged)?;
    let count = data.get(33..37).ok_or_else(damaged)?;
    let sign_count = u32::from_be_bytes(count.try_into().map_err(|_| damaged())?);
    if flags & FLAG_USER_PRESENT == 0 || flags & FLAG_USER_VERIFIED == 0 {
        return Err(AppError::bad_request(
            "The passkey didn't confirm it was you. Unlock it with your fingerprint, face or PIN.",
        ));
    }
    Ok(AuthData {
        flags,
        sign_count,
        rest: data.get(37..).unwrap_or_default(),
    })
}

/// A new passkey, as the browser describes it.
#[derive(Deserialize)]
pub struct Registration {
    pub ceremony: String,
    /// `rawId`, base64url.
    pub id: String,
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    #[serde(rename = "authenticatorData")]
    pub authenticator_data: String,
    /// SPKI DER, base64url.
    #[serde(rename = "publicKey")]
    pub public_key: String,
    #[serde(rename = "publicKeyAlgorithm")]
    pub algorithm: i64,
    #[serde(default)]
    pub transports: Vec<String>,
    #[serde(default)]
    pub name: String,
}

/// A checked passkey, ready to store.
#[derive(Debug, Clone)]
pub struct NewPasskey {
    pub credential_id: Vec<u8>,
    pub public_key: Vec<u8>,
    pub algorithm: i64,
    pub sign_count: u32,
    pub transports: Vec<String>,
}

pub fn verify_registration(
    registration: &Registration,
    challenge: &[u8],
    site: &Site,
) -> AppResult<NewPasskey> {
    let damaged = || AppError::bad_request("The passkey answer was damaged.");
    check_client_data(
        &decode(&registration.client_data_json)?,
        "webauthn.create",
        challenge,
        site,
    )?;
    let auth_data = decode(&registration.authenticator_data)?;
    let parsed = parse_auth_data(&auth_data, site)?;
    if parsed.flags & FLAG_ATTESTED == 0 {
        return Err(damaged());
    }
    // AAGUID (16 bytes), then the credential id with its length.
    let length = parsed.rest.get(16..18).ok_or_else(damaged)?;
    let length = usize::from(u16::from_be_bytes([
        length.first().copied().unwrap_or(0),
        length.get(1).copied().unwrap_or(0),
    ]));
    let credential_id = parsed
        .rest
        .get(18..18_usize.saturating_add(length))
        .ok_or_else(damaged)?;
    if credential_id != decode(&registration.id)?.as_slice() || credential_id.is_empty() {
        return Err(damaged());
    }
    if !ALGORITHMS.contains(&registration.algorithm) {
        return Err(AppError::bad_request(
            "This passkey uses a kind of key Sideporch doesn't support.",
        ));
    }
    let public_key = decode(&registration.public_key)?;
    // Parsing proves the key matches its algorithm.
    key_for(registration.algorithm, &public_key)?;
    Ok(NewPasskey {
        credential_id: credential_id.to_vec(),
        public_key,
        algorithm: registration.algorithm,
        sign_count: parsed.sign_count,
        transports: registration
            .transports
            .iter()
            .filter(|transport| transport.len() <= 16)
            .take(8)
            .cloned()
            .collect(),
    })
}

/// A sign-in answer from the browser.
#[derive(Deserialize)]
pub struct Assertion {
    pub ceremony: String,
    pub id: String,
    #[serde(rename = "clientDataJSON")]
    pub client_data_json: String,
    #[serde(rename = "authenticatorData")]
    pub authenticator_data: String,
    pub signature: String,
}

/// A stored passkey, to check an assertion against.
#[derive(Debug, Clone)]
pub struct Stored {
    pub id: i64,
    pub user_id: i64,
    pub public_key: Vec<u8>,
    pub algorithm: i64,
    pub sign_count: u32,
}

/// Checks a sign-in and returns the new signature counter.
pub fn verify_assertion(
    assertion: &Assertion,
    stored: &Stored,
    challenge: &[u8],
    site: &Site,
) -> AppResult<u32> {
    let client_data = decode(&assertion.client_data_json)?;
    check_client_data(&client_data, "webauthn.get", challenge, site)?;
    let auth_data = decode(&assertion.authenticator_data)?;
    let parsed = parse_auth_data(&auth_data, site)?;
    let mut signed = auth_data.clone();
    signed.extend_from_slice(Sha256::digest(&client_data).as_slice());
    let key = key_for(stored.algorithm, &stored.public_key)?;
    key.verify(&signed, &decode(&assertion.signature)?)
        .map_err(|_| AppError::bad_request("That passkey didn't match."))?;
    // Counters only mean something when the device keeps one; a counter
    // that went backwards points at a copied key.
    if parsed.sign_count != 0 && stored.sign_count != 0 && parsed.sign_count <= stored.sign_count {
        return Err(AppError::bad_request(
            "This passkey looks copied. Remove it and add it again.",
        ));
    }
    Ok(parsed.sign_count)
}

const OID_EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_ED25519: &[u8] = &[0x2b, 0x65, 0x70];
const OID_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];

/// Reads one DER element: its tag, its contents and what follows.
fn der(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *input.first()?;
    let first = *input.get(1)?;
    let (length, header) = if first < 0x80 {
        (usize::from(first), 2_usize)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 2 {
            return None;
        }
        let bytes = input.get(2..2_usize.saturating_add(count))?;
        let length = bytes
            .iter()
            .fold(0_usize, |length, byte| (length << 8) | usize::from(*byte));
        (length, 2_usize.saturating_add(count))
    };
    let end = header.checked_add(length)?;
    Some((tag, input.get(header..end)?, input.get(end..)?))
}

/// A public key from SPKI DER, checked against its COSE algorithm.
fn key_for(algorithm: i64, spki: &[u8]) -> AppResult<signature::UnparsedPublicKey<Vec<u8>>> {
    let bad = || AppError::bad_request("The passkey's public key is damaged.");
    let (tag, body, rest) = der(spki).ok_or_else(bad)?;
    if tag != 0x30 || !rest.is_empty() {
        return Err(bad());
    }
    let (tag, identifier, rest) = der(body).ok_or_else(bad)?;
    if tag != 0x30 {
        return Err(bad());
    }
    let (tag, key, _) = der(rest).ok_or_else(bad)?;
    if tag != 0x03 || key.first() != Some(&0) {
        return Err(bad());
    }
    let key = key.get(1..).ok_or_else(bad)?.to_vec();
    let (tag, oid, parameters) = der(identifier).ok_or_else(bad)?;
    if tag != 0x06 {
        return Err(bad());
    }
    let curve = der(parameters).map(|(_, curve, _)| curve);
    let (expected, verifier): (bool, &'static dyn signature::VerificationAlgorithm) =
        match algorithm {
            ES256 => (
                oid == OID_EC && curve == Some(OID_P256) && key.len() == 65,
                &signature::ECDSA_P256_SHA256_ASN1,
            ),
            EDDSA => (oid == OID_ED25519 && key.len() == 32, &signature::ED25519),
            RS256 => (oid == OID_RSA, &signature::RSA_PKCS1_2048_8192_SHA256),
            _ => (false, &signature::ED25519),
        };
    if !expected {
        return Err(bad());
    }
    Ok(signature::UnparsedPublicKey::new(verifier, key))
}

/// A name for a new passkey from the device's transports, if the person
/// gave none.
pub fn default_name(transports: &[String]) -> String {
    if transports.iter().any(|transport| transport == "hybrid") {
        "Passkey on a phone".to_owned()
    } else if transports.iter().any(|transport| transport == "internal") {
        "Passkey on this device".to_owned()
    } else if transports
        .iter()
        .any(|transport| transport == "usb" || transport == "nfc")
    {
        "Security key".to_owned()
    } else {
        "Passkey".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use ring::{
        rand::SystemRandom,
        signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
    };

    use super::*;

    /// SPKI DER for a P-256 public key.
    fn p256_spki(point: &[u8]) -> Vec<u8> {
        let mut spki = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        spki.extend_from_slice(point);
        spki
    }

    fn site() -> Site {
        Site::from_base_url("https://chat.example.com")
    }

    fn client_data(kind: &str, challenge: &[u8], origin: &str) -> String {
        encode(
            serde_json::json!({ "type": kind, "challenge": encode(challenge), "origin": origin })
                .to_string()
                .as_bytes(),
        )
    }

    fn auth_data(rp_id: &str, flags: u8, count: u32, credential: Option<&[u8]>) -> Vec<u8> {
        let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
        data.push(flags);
        data.extend_from_slice(&count.to_be_bytes());
        if let Some(credential) = credential {
            data.extend_from_slice(&[0; 16]);
            data.extend_from_slice(&u16::try_from(credential.len()).unwrap().to_be_bytes());
            data.extend_from_slice(credential);
            // A COSE key would follow; it isn't read.
            data.extend_from_slice(&[0xa0]);
        }
        data
    }

    #[test]
    fn sites_from_base_urls() {
        let site = Site::from_base_url("http://localhost:8080");
        assert_eq!(
            (site.rp_id.as_str(), site.origin.as_str()),
            ("localhost", "http://localhost:8080")
        );
        assert_eq!(Site::from_base_url("https://[::1]:8443").rp_id, "::1");
    }

    #[test]
    fn registers_and_signs_in_with_a_p256_key() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let credential = b"credential-1";
        let challenge = b"register challenge";
        let registration = Registration {
            ceremony: String::new(),
            id: encode(credential),
            client_data_json: client_data("webauthn.create", challenge, "https://chat.example.com"),
            authenticator_data: encode(&auth_data("chat.example.com", 0x45, 0, Some(credential))),
            public_key: encode(&p256_spki(pair.public_key().as_ref())),
            algorithm: -7,
            transports: vec!["internal".to_owned()],
            name: String::new(),
        };
        let passkey = verify_registration(&registration, challenge, &site()).unwrap();
        assert_eq!(passkey.credential_id, credential);

        // The wrong origin, site or challenge fails.
        let mut wrong = registration;
        wrong.client_data_json = client_data("webauthn.create", challenge, "https://evil.example");
        assert!(verify_registration(&wrong, challenge, &site()).is_err());
        wrong.client_data_json =
            client_data("webauthn.create", challenge, "https://chat.example.com");
        assert!(verify_registration(&wrong, b"other", &site()).is_err());
        wrong.authenticator_data = encode(&auth_data("evil.example", 0x45, 0, Some(credential)));
        assert!(verify_registration(&wrong, challenge, &site()).is_err());
        // Without user verification, it fails too.
        wrong.authenticator_data =
            encode(&auth_data("chat.example.com", 0x41, 0, Some(credential)));
        assert!(verify_registration(&wrong, challenge, &site()).is_err());

        let stored = Stored {
            id: 1,
            user_id: 1,
            public_key: passkey.public_key,
            algorithm: -7,
            sign_count: 0,
        };
        let challenge = b"sign in challenge";
        let client = client_data("webauthn.get", challenge, "https://chat.example.com");
        let data = auth_data("chat.example.com", 0x05, 7, None);
        let mut signed = data.clone();
        signed.extend_from_slice(&Sha256::digest(decode(&client).unwrap()));
        let signature = pair.sign(&rng, &signed).unwrap();
        let assertion = Assertion {
            ceremony: String::new(),
            id: encode(credential),
            client_data_json: client,
            authenticator_data: encode(&data),
            signature: encode(signature.as_ref()),
        };
        assert_eq!(
            verify_assertion(&assertion, &stored, challenge, &site()).unwrap(),
            7
        );
        // A counter that doesn't go up means a copied key.
        let seen = Stored {
            sign_count: 7,
            ..stored.clone()
        };
        assert!(verify_assertion(&assertion, &seen, challenge, &site()).is_err());
        // A signature over something else fails.
        let forged = Assertion {
            signature: encode(pair.sign(&rng, b"other").unwrap().as_ref()),
            ..assertion
        };
        assert!(verify_assertion(&forged, &stored, challenge, &site()).is_err());
    }

    #[test]
    fn keys_must_match_their_algorithm() {
        let point = [4_u8; 65];
        assert!(key_for(-7, &p256_spki(&point)).is_ok());
        assert!(key_for(-8, &p256_spki(&point)).is_err());
        assert!(key_for(-7, &[0x30, 0x03, 0x02, 0x01, 0x00]).is_err());
        assert!(key_for(-7, &[]).is_err());
    }

    #[test]
    fn ceremonies_work_once_for_their_purpose() {
        let ceremonies = Ceremonies::default();
        let (id, challenge) = ceremonies.start(Purpose::Register(3), 0).unwrap();
        assert!(ceremonies.finish(&id, &Purpose::SignIn, 1).is_err());
        let (id, _) = ceremonies.start(Purpose::Register(3), 0).unwrap();
        assert_eq!(
            ceremonies
                .finish(&id, &Purpose::Register(3), 1)
                .unwrap()
                .len(),
            challenge.len()
        );
        assert!(ceremonies.finish(&id, &Purpose::Register(3), 1).is_err());
        let (id, _) = ceremonies.start(Purpose::SignIn, 0).unwrap();
        assert!(
            ceremonies
                .finish(&id, &Purpose::SignIn, CEREMONY_MS + 1)
                .is_err()
        );
    }
}
