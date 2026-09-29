//! Checking that release checksums were signed with Sideporch's release key.
//!
//! Releases publish `SHA256SUMS` and `SHA256SUMS.minisig`, a
//! [minisign](https://jedisct1.github.io/minisign/) signature made in CI
//! with a key only the release workflow holds. The public key is built into
//! Sideporch, so an update is only installed when the checksums come from
//! a release, and the archive matches them.

/// The public key that signs releases, as minisign prints it.
pub const RELEASE_KEY: &str = "RWRtn2cj2SpGnZDtKTNsgnc8mv68NfwlwFgA+hcmOD+cWH8dSQxGkuoo";

/// Checks `signature` (a `.minisig` file) over `data` with `public_key`,
/// and that its trusted comment names `version`, so a signature from
/// another release can't be passed off for this one.
///
/// # Errors
///
/// Says why the signature doesn't hold.
pub fn verify(public_key: &str, data: &[u8], signature: &str, version: &str) -> Result<(), String> {
    let key = minisign_verify::PublicKey::from_base64(public_key.trim())
        .map_err(|_| "the release key isn't valid".to_owned())?;
    let signature = minisign_verify::Signature::decode(signature)
        .map_err(|_| "the release signature can't be read".to_owned())?;
    // Prehashed signatures only, as current minisign makes them.
    key.verify(data, &signature, false).map_err(|_| {
        "the release checksums aren't signed with Sideporch's release key".to_owned()
    })?;
    let expected = format!("sideporch {version}");
    let comment = signature.trusted_comment();
    if comment == expected || comment.starts_with(&format!("{expected} ")) {
        Ok(())
    } else {
        Err(format!(
            "the signature is for “{comment}”, not Sideporch {version}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use base64ct::{Base64, Encoding as _};
    use blake2::Digest as _;
    use ring::signature::{Ed25519KeyPair, KeyPair as _};

    use super::verify;

    /// Signs like `minisign -S` (prehashed with BLAKE2b-512), for tests.
    /// Returns the public key and the `.minisig` text.
    fn sign(seed: u8, data: &[u8], trusted_comment: &str) -> (String, String) {
        let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
        let keynum = [seed, 1, 2, 3, 4, 5, 6, 7];
        let public = [b"Ed".as_slice(), &keynum, pair.public_key().as_ref()].concat();
        let hash = blake2::Blake2b512::digest(data);
        let signature = pair.sign(&hash);
        let line = [b"ED".as_slice(), &keynum, signature.as_ref()].concat();
        let global = pair.sign(&[signature.as_ref(), trusted_comment.as_bytes()].concat());
        let text = format!(
            "untrusted comment: signature from a test key\n{}\ntrusted comment: {trusted_comment}\n{}\n",
            Base64::encode_string(&line),
            Base64::encode_string(global.as_ref()),
        );
        (Base64::encode_string(&public), text)
    }

    #[test]
    fn accepts_the_release_signature() {
        let (key, signature) = sign(7, b"abc  sideporch-0.5.0-x.tar.gz\n", "sideporch 0.5.0");
        assert_eq!(
            verify(
                &key,
                b"abc  sideporch-0.5.0-x.tar.gz\n",
                &signature,
                "0.5.0"
            ),
            Ok(())
        );
    }

    #[test]
    fn refuses_changed_data_other_keys_and_other_releases() {
        let data = b"abc  sideporch-0.5.0-x.tar.gz\n";
        let (key, signature) = sign(7, data, "sideporch 0.5.0");
        assert!(
            verify(
                &key,
                b"abd  sideporch-0.5.0-x.tar.gz\n",
                &signature,
                "0.5.0"
            )
            .is_err()
        );
        let (other_key, _) = sign(8, data, "sideporch 0.5.0");
        assert!(verify(&other_key, data, &signature, "0.5.0").is_err());
        assert!(
            verify(&key, data, &signature, "0.5.1").is_err(),
            "signed for another version"
        );
        let (_, forged) = sign(8, data, "sideporch 0.5.0");
        assert!(verify(&key, data, &forged, "0.5.0").is_err());
        assert!(verify(&key, data, "not a signature", "0.5.0").is_err());
        assert!(verify("not a key", data, &signature, "0.5.0").is_err());
    }

    /// Signed with the real release key by scripts/release-sign.sh, as the
    /// release workflow signs, so the key built in matches the one CI holds
    /// (as far as this machine could tell) and minisign's format is read right.
    #[test]
    fn accepts_what_the_release_script_signs() {
        let sums = include_bytes!("../../tests/fixtures/release/SHA256SUMS");
        let signature = include_str!("../../tests/fixtures/release/SHA256SUMS.minisig");
        assert_eq!(verify(super::RELEASE_KEY, sums, signature, "0.0.1"), Ok(()));
        assert!(verify(super::RELEASE_KEY, sums, signature, "0.0.2").is_err());
    }

    #[test]
    fn the_built_in_key_is_valid() {
        assert!(minisign_verify::PublicKey::from_base64(super::RELEASE_KEY).is_ok());
    }
}
