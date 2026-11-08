//! The automation store's catalogue: one JSON index that lists automation
//! files people publish, which admins browse and install with a click.
//!
//! The index is a static file (on sideporch.app by default). Each entry
//! names a `.sideporch.json` file, its SHA-256 and, optionally, a minisign
//! signature of the file. Sideporch reads the index through the guarded HTTP
//! client, caches it briefly, and checks each file against its entry before
//! previewing it. Sideporch doesn't review what's listed.

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use super::{Settings, http};
use crate::{AppState, store, updates};

/// What the index says it is.
pub const FORMAT: &str = "sideporch-catalogue";
/// The index format this Sideporch reads. Later versions are refused.
pub const VERSION: u32 = 1;
/// Where the store is unless an admin points it somewhere else.
pub const DEFAULT_URL: &str = "https://sideporch.app/automations/index.json";
/// How long a fetched index is reused before it is read again.
const CACHE_FOR: Duration = Duration::from_mins(10);
/// The largest index Sideporch reads, in bytes.
pub const MAX_INDEX_BYTES: usize = 512 * 1024;
/// The most entries one index may list.
const MAX_ENTRIES: usize = 500;
/// The largest signature file Sideporch reads, in bytes.
pub const MAX_SIGNATURE_BYTES: usize = 16 * 1024;

/// One automation the store lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    pub version: String,
    /// Where the `.sideporch.json` file is.
    pub file: String,
    /// SHA-256 of the file, in hex.
    pub sha256: String,
    /// Where a minisign signature of the file is, when someone signed it.
    #[serde(default)]
    pub signature: Option<String>,
}

#[derive(Deserialize)]
struct Index {
    format: String,
    version: u32,
    entries: Vec<Entry>,
}

/// Reads an index, or says why it can't be used.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    if text.len() > MAX_INDEX_BYTES {
        return Err("the index is larger than 512 KB".to_owned());
    }
    let index: Index = serde_json::from_str(text)
        .map_err(|_| "the file isn't a Sideporch automation catalogue".to_owned())?;
    if index.format != FORMAT {
        return Err("the file isn't a Sideporch automation catalogue".to_owned());
    }
    if index.version == 0 || index.version > VERSION {
        return Err(format!(
            "the catalogue is format version {}, and this Sideporch reads version {VERSION}; update Sideporch to use it",
            index.version
        ));
    }
    if index.entries.len() > MAX_ENTRIES {
        return Err(format!(
            "the catalogue lists more than {MAX_ENTRIES} automations"
        ));
    }
    let mut entries = Vec::with_capacity(index.entries.len());
    for mut entry in index.entries {
        entry.name = entry.name.trim().chars().take(80).collect();
        if entry.name.is_empty() {
            return Err("an entry has no name".to_owned());
        }
        let version = entry.version.trim().to_owned();
        entry.version = version;
        if entry.version.is_empty() {
            return Err(format!("{} has no version", entry.name));
        }
        if !is_web_address(&entry.file) {
            return Err(format!("{} has no file address", entry.name));
        }
        entry.sha256 = entry.sha256.trim().to_ascii_lowercase();
        if entry.sha256.len() != 64 || !entry.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("{} has no valid SHA-256", entry.name));
        }
        if let Some(signature) = &entry.signature
            && !is_web_address(signature)
        {
            return Err(format!(
                "{} has a signature address that isn't one",
                entry.name
            ));
        }
        entries.push(entry);
    }
    Ok(entries)
}

fn is_web_address(text: &str) -> bool {
    text.starts_with("https://") || text.starts_with("http://")
}

/// SHA-256 of `bytes`, in lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            use std::fmt::Write as _;
            // Writing to a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Checks that `signature` was made with Sideporch's key over `file`, and
/// that it names this file's checksum in its trusted comment, so a signature
/// for one file, or for a release, can't be passed off for another.
pub fn check_signature(
    key: &str,
    file: &[u8],
    sha256: &str,
    signature: &str,
) -> Result<(), String> {
    let comment = updates::signature::trusted_comment(key, file, signature)
        .map_err(|error| format!("the signature doesn't check out ({error})"))?;
    if comment.starts_with(&format!("sideporch-automation {sha256}")) {
        Ok(())
    } else {
        Err("the signature is for another file".to_owned())
    }
}

/// Whether `available` is a newer version than `installed`. Compares the
/// numbers in each dotted version (`1.10.0` is newer than `1.9.2`); a
/// version without numbers is only newer when it differs.
pub fn is_newer(installed: &str, available: &str) -> bool {
    fn numbers(version: &str) -> Vec<u64> {
        version
            .split(['.', '-', '+'])
            .map_while(|part| part.parse().ok())
            .collect()
    }
    let (old, new) = (numbers(installed), numbers(available));
    if old.is_empty() || new.is_empty() {
        return installed != available;
    }
    let width = old.len().max(new.len());
    let padded = |numbers: &[u64]| -> Vec<u64> {
        (0..width)
            .map(|index| numbers.get(index).copied().unwrap_or(0))
            .collect()
    };
    padded(&new) > padded(&old)
}

/// The store's newer versions of installed automations, by id: an installed
/// automation is offered the entry whose file it came from, when that entry
/// has a newer version.
pub fn newer_versions(
    installed: &HashMap<i64, store::AutomationOrigin>,
    entries: &[Entry],
) -> HashMap<i64, Entry> {
    installed
        .iter()
        .filter_map(|(id, origin)| {
            let entry = entries.iter().find(|entry| entry.file == origin.url)?;
            let version = origin.version.as_deref().unwrap_or_default();
            is_newer(version, &entry.version).then(|| (*id, entry.clone()))
        })
        .collect()
}

/// The index as last fetched, reused for a few minutes.
#[derive(Default)]
pub struct Cache {
    fetched: Mutex<Option<Fetched>>,
}

struct Fetched {
    url: String,
    at: Instant,
    entries: Vec<Entry>,
}

impl Cache {
    /// The entries fetched from `url` a moment ago, if still fresh.
    pub fn get(&self, url: &str) -> Option<Vec<Entry>> {
        self.fetched.lock().ok().and_then(|guard| {
            guard
                .as_ref()
                .filter(|fetched| fetched.url == url && fetched.at.elapsed() < CACHE_FOR)
                .map(|fetched| fetched.entries.clone())
        })
    }

    pub fn put(&self, url: &str, entries: Vec<Entry>) {
        if let Ok(mut guard) = self.fetched.lock() {
            *guard = Some(Fetched {
                url: url.to_owned(),
                at: Instant::now(),
                entries,
            });
        }
    }
}

/// The store's entries: from the cache, or fetched from the index's address
/// through the guarded client. Errors say what couldn't be reached or read.
pub async fn index(state: &AppState, settings: &Settings) -> Result<Vec<Entry>, String> {
    let url = settings.catalogue().to_owned();
    if let Some(entries) = state.automations.catalogue_cache().get(&url) {
        return Ok(entries);
    }
    let body = http::download(settings.allow_private_network, &url, MAX_INDEX_BYTES)
        .await
        .map_err(|error| format!("The store at {url} can't be reached: {error}."))?;
    let entries = parse(&String::from_utf8_lossy(&body))
        .map_err(|error| format!("The store at {url} can't be read: {error}."))?;
    state
        .automations
        .catalogue_cache()
        .put(&url, entries.clone());
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automations::{bundle, tooling};

    const INDEX: &str = r#"{"format":"sideporch-catalogue","version":1,"entries":[
        {"name":"Weather","description":"Posts the forecast","author":"Ann","license":"MIT",
         "version":"1.2.0","file":"https://example.com/weather.sideporch.json",
         "sha256":"AB12AB12AB12AB12AB12AB12AB12AB12AB12AB12AB12AB12AB12AB12AB12AB12"}]}"#;

    #[test]
    fn reads_entries_and_normalizes_the_checksum() {
        let entries = parse(INDEX).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "Weather");
        assert_eq!(entries[0].sha256, "ab12".repeat(16));
        assert_eq!(entries[0].signature, None);
    }

    #[test]
    fn refuses_other_files_newer_formats_and_bad_entries() {
        assert!(parse("{}").is_err());
        assert!(parse(&INDEX.replace("sideporch-catalogue", "other")).is_err());
        assert!(parse(&INDEX.replace("\"version\":1", "\"version\":2")).is_err());
        assert!(parse(&INDEX.replace("https://example.com/weather", "ftp://x")).is_err());
        assert!(parse(&INDEX.replace("\"sha256\":\"AB12", "\"sha256\":\"short")).is_err());
        assert!(parse(&INDEX.replace("\"name\":\"Weather\"", "\"name\":\"  \"")).is_err());
    }

    #[test]
    fn compares_versions_by_their_numbers() {
        assert!(is_newer("1.9.2", "1.10.0"));
        assert!(is_newer("1.2", "1.2.1"));
        assert!(!is_newer("1.2.0", "1.2.0"));
        assert!(!is_newer("2.0.0", "1.9.9"));
        assert!(is_newer("nightly-a", "nightly-b"));
    }

    #[test]
    fn offers_the_newer_entry_of_what_was_installed_from_it() {
        let entries = parse(INDEX).unwrap();
        let origin = |url: &str, version: &str| store::AutomationOrigin {
            url: url.to_owned(),
            version: Some(version.to_owned()),
            sha256: None,
        };
        let installed = HashMap::from([
            (
                1,
                origin("https://example.com/weather.sideporch.json", "1.1.0"),
            ),
            (
                2,
                origin("https://example.com/weather.sideporch.json", "1.2.0"),
            ),
            (3, origin("https://example.com/elsewhere.json", "0.1.0")),
        ]);
        let offered = newer_versions(&installed, &entries);
        assert_eq!(offered.len(), 1);
        assert_eq!(
            offered.get(&1).map(|entry| entry.version.as_str()),
            Some("1.2.0")
        );
    }

    #[test]
    fn hashes_like_sha256sum() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    /// The example files the website publishes: each one matches its index
    /// entry, reads as a bundle, and lints without errors.
    #[test]
    fn the_published_examples_are_valid() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("website/static/automations");
        let index = std::fs::read_to_string(dir.join("index.json")).unwrap();
        let entries = parse(&index).unwrap();
        assert!(!entries.is_empty());
        for entry in entries {
            let name = entry.file.rsplit('/').next().unwrap().to_owned();
            let text = std::fs::read_to_string(dir.join(&name)).unwrap();
            assert_eq!(sha256_hex(text.as_bytes()), entry.sha256, "{name}");
            assert_eq!(
                entry.signature, None,
                "{name} is unsigned until the maintainer signs it"
            );
            let bundle = bundle::parse(&text).unwrap();
            for item in bundle.items {
                let errors = tooling::lint(&item.source)
                    .into_iter()
                    .filter(|problem| problem.severity == tooling::Severity::Error)
                    .count();
                assert_eq!(errors, 0, "{name}: {}", item.name);
            }
        }
    }
}
