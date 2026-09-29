//! Codes from authenticator apps (TOTP, RFC 6238): six digits that change
//! every 30 seconds, from a secret shared once through a QR code.

use ring::hmac;

use crate::error::{AppError, AppResult};

const STEP_SECONDS: i64 = 30;
const DIGITS: u32 = 6;
const ISSUER: &str = "Sideporch";

/// A new random secret, 160 bits as RFC 4226 recommends.
pub fn new_secret() -> AppResult<Vec<u8>> {
    let mut secret = vec![0_u8; 20];
    getrandom::fill(&mut secret).map_err(AppError::internal)?;
    Ok(secret)
}

/// RFC 4648 base32 without padding, as authenticator apps expect.
pub fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0_u32;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits = bits.saturating_add(8);
        while bits >= 5 {
            bits = bits.saturating_sub(5);
            let index = usize::try_from((buffer >> bits) & 31).unwrap_or(0);
            out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
        }
    }
    if bits > 0 {
        let index = usize::try_from((buffer << 5_u32.saturating_sub(bits)) & 31).unwrap_or(0);
        out.push(char::from(ALPHABET.get(index).copied().unwrap_or(b'A')));
    }
    out
}

/// Reads base32, ignoring spaces, case and padding.
pub fn from_base32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u32 = 0;
    let mut bits = 0_u32;
    for c in text.chars().filter(|c| !c.is_whitespace() && *c != '=') {
        let value = match c.to_ascii_uppercase() {
            letter @ 'A'..='Z' => u32::from(letter).saturating_sub(u32::from('A')),
            digit @ '2'..='7' => u32::from(digit)
                .saturating_sub(u32::from('2'))
                .saturating_add(26),
            _ => return None,
        };
        buffer = (buffer << 5) | value;
        bits = bits.saturating_add(5);
        if bits >= 8 {
            bits = bits.saturating_sub(8);
            out.push(u8::try_from((buffer >> bits) & 0xFF).ok()?);
        }
    }
    Some(out)
}

/// The `otpauth://` link apps read from the QR code.
pub fn uri(secret: &[u8], account: &str) -> String {
    let encode = |value: &str| {
        value
            .bytes()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
                    char::from(byte).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect::<String>()
    };
    format!(
        "otpauth://totp/{ISSUER}:{}?secret={}&issuer={ISSUER}&algorithm=SHA1&digits={DIGITS}&period={STEP_SECONDS}",
        encode(account),
        base32(secret)
    )
}

/// The code for one 30-second step (HOTP, RFC 4226).
pub fn code_at(secret: &[u8], step: u64) -> u32 {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &step.to_be_bytes());
    let digest = tag.as_ref();
    let offset = usize::from(digest.last().copied().unwrap_or(0) & 0x0F);
    let part = digest
        .get(offset..offset.saturating_add(4))
        .unwrap_or(&[0; 4]);
    let number = u32::from_be_bytes([
        part.first().copied().unwrap_or(0) & 0x7F,
        part.get(1).copied().unwrap_or(0),
        part.get(2).copied().unwrap_or(0),
        part.get(3).copied().unwrap_or(0),
    ]);
    number.checked_rem(1_000_000).unwrap_or(0)
}

/// The step a moment in milliseconds falls in.
pub fn step(now_ms: i64) -> u64 {
    u64::try_from(now_ms.checked_div(1000 * STEP_SECONDS).unwrap_or(0)).unwrap_or(0)
}

/// Checks a typed code against the current step and its neighbours, for
/// clocks that are a little off. A step already used can't be used again.
/// Returns the step the code belongs to.
pub fn verify(secret: &[u8], code: &str, now_ms: i64, last_used: u64) -> Option<u64> {
    let digits: String = code.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != usize::try_from(DIGITS).unwrap_or(6) {
        return None;
    }
    let typed: u32 = digits.parse().ok()?;
    let now = step(now_ms);
    [now.saturating_sub(1), now, now.saturating_add(1)]
        .into_iter()
        .filter(|candidate| *candidate > last_used)
        .find(|candidate| {
            crate::auth::same_bytes(
                &code_at(secret, *candidate).to_be_bytes(),
                &typed.to_be_bytes(),
            )
        })
}

/// Ten single-use recovery codes, like `k7m2-9xq4`.
pub fn recovery_codes() -> AppResult<Vec<String>> {
    const ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut codes = Vec::with_capacity(10);
    for _ in 0..10 {
        let mut bytes = [0_u8; 8];
        getrandom::fill(&mut bytes).map_err(AppError::internal)?;
        let letters: String = bytes
            .iter()
            .map(|byte| {
                let index = usize::from(*byte).checked_rem(ALPHABET.len()).unwrap_or(0);
                char::from(ALPHABET.get(index).copied().unwrap_or(b'a'))
            })
            .collect();
        let (first, second) = letters.split_at(4);
        codes.push(format!("{first}-{second}"));
    }
    Ok(codes)
}

/// Recovery codes are compared without case, spaces or dashes.
pub fn normalize_recovery(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_rfc_test_vectors() {
        // RFC 6238, appendix B, SHA-1, truncated to six digits.
        let secret = b"12345678901234567890";
        for (seconds, code) in [
            (59_i64, 287_082),
            (1_111_111_109, 81_804),
            (1_234_567_890, 5_924),
            (2_000_000_000, 279_037),
        ] {
            assert_eq!(code_at(secret, step(seconds * 1000)), code, "at {seconds}");
        }
    }

    #[test]
    fn codes_work_once_and_near_now() {
        let secret = b"12345678901234567890";
        let now = 59_000;
        assert_eq!(verify(secret, "287 082", now, 0), Some(1));
        // The same step can't be used twice.
        assert_eq!(verify(secret, "287082", now, 1), None);
        // A code from the previous step still works, not one from before.
        let now = 3_000_000;
        let previous = format!("{:06}", code_at(secret, 99));
        assert_eq!(verify(secret, &previous, now, 0), Some(99));
        let older = format!("{:06}", code_at(secret, 98));
        assert_eq!(verify(secret, &older, now, 0), None);
        assert_eq!(verify(secret, "12345", now, 0), None);
    }

    #[test]
    fn base32_round_trips() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
        assert_eq!(
            from_base32("mzxw 6ytb oi==").as_deref(),
            Some(&b"foobar"[..])
        );
        let secret = new_secret().unwrap();
        assert_eq!(from_base32(&base32(&secret)), Some(secret));
        assert!(
            uri(b"foobar", "ada lovelace").contains("Sideporch:ada%20lovelace?secret=MZXW6YTBOI")
        );
    }

    #[test]
    fn recovery_codes_look_alike_and_compare_loosely() {
        let codes = recovery_codes().unwrap();
        assert_eq!(codes.len(), 10);
        assert!(
            codes
                .iter()
                .all(|code| code.len() == 9 && code.as_bytes()[4] == b'-')
        );
        assert_eq!(normalize_recovery(" K7M2-9XQ4 "), "k7m29xq4");
    }
}
