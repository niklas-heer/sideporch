//! Releases as GitHub lists them, and how urgent the newer ones are.

use serde::{Deserialize, Serialize};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// From here on a newer version shows a reminder that can be hidden for a week.
pub const REMIND_AFTER_DAYS: i64 = 14;
/// From here on the reminder comes back every day.
pub const OVERDUE_AFTER_DAYS: i64 = 60;

/// A release version, `MAJOR.MINOR.PATCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// Reads `1.2.3` or `v1.2.3`. Pre-releases (`1.2.3-rc.1`) don't count.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let mut parts = text.split('.');
        let version = Self(
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        );
        parts.next().is_none().then_some(version)
    }

    /// The version of this program.
    #[must_use]
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self(0, 0, 0))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A published release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: Version,
    /// When it was published, in milliseconds.
    pub published_at: i64,
    /// Its release notes have a Security section.
    pub security: bool,
    /// The release page.
    pub url: String,
}

/// A release as GitHub's API describes it.
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: String,
}

/// Reads GitHub's list of releases, leaving out drafts, pre-releases and
/// tags that aren't versions. Newest first.
///
/// # Errors
///
/// Fails if the answer isn't a list of releases.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    let listed: Vec<GitHubRelease> = serde_json::from_str(json)
        .map_err(|error| format!("unexpected answer from GitHub: {error}"))?;
    let mut releases: Vec<Release> = listed
        .into_iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| {
            let published_at = release
                .published_at
                .as_deref()?
                .parse::<jiff::Timestamp>()
                .ok()?
                .as_millisecond();
            Some(Release {
                version: Version::parse(&release.tag_name)?,
                published_at,
                security: release.body.as_deref().is_some_and(mentions_security),
                url: release.html_url,
            })
        })
        .collect();
    releases.sort_by_key(|release| std::cmp::Reverse(release.version));
    Ok(releases)
}

/// Whether release notes have a Security section, which `fix(security)`
/// commits produce (see cliff.toml).
fn mentions_security(notes: &str) -> bool {
    notes.lines().any(|line| {
        line.trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case("security")
            && line.starts_with('#')
    })
}

/// The releases newer than `current`, newest first.
#[must_use]
pub fn newer_than(releases: &[Release], current: Version) -> Vec<Release> {
    releases
        .iter()
        .filter(|release| release.version > current)
        .cloned()
        .collect()
}

/// How pressing it is to update, from the releases newer than the running one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    /// A newer version exists; the Updates page says so.
    Available,
    /// It has been out for a while: remind admins, hidden for a week at a time.
    Remind,
    /// Long out of date: remind admins every day.
    Overdue,
    /// A newer release fixes a security problem: remind admins every day.
    Security,
}

impl Urgency {
    /// From the releases newer than the running one; `None` when up to date.
    #[must_use]
    pub fn of(newer: &[Release], now: i64) -> Option<Self> {
        if newer.iter().any(|release| release.security) {
            return Some(Self::Security);
        }
        let first_out = newer.iter().map(|release| release.published_at).min()?;
        let days = now.saturating_sub(first_out) / DAY_MS;
        Some(if days >= OVERDUE_AFTER_DAYS {
            Self::Overdue
        } else if days >= REMIND_AFTER_DAYS {
            Self::Remind
        } else {
            Self::Available
        })
    }

    /// For how long an admin can hide the reminder; `None` when there is none.
    #[must_use]
    pub const fn hide_for_days(self) -> Option<i64> {
        match self {
            Self::Available => None,
            Self::Remind => Some(7),
            Self::Overdue | Self::Security => Some(1),
        }
    }

    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Remind => "remind",
            Self::Overdue => "overdue",
            Self::Security => "security",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = DAY_MS;

    fn release(version: &str, published_at: i64, security: bool) -> Release {
        Release {
            version: Version::parse(version).unwrap(),
            published_at,
            security,
            url: String::new(),
        }
    }

    #[test]
    fn versions_compare_by_number() {
        assert_eq!(Version::parse("v0.10.0"), Some(Version(0, 10, 0)));
        assert!(Version::parse("0.10.0").unwrap() > Version::parse("0.9.3").unwrap());
        assert_eq!(Version::parse("1.2.3-rc.1"), None);
        assert_eq!(Version::parse("1.2"), None);
        assert_eq!(Version::parse("1.2.3.4"), None);
        assert_eq!(Version(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn reads_github_releases() {
        let json = r####"[
          {"tag_name": "v0.6.0-rc.1", "prerelease": true, "published_at": "2026-10-03T10:00:00Z", "body": "", "html_url": "x"},
          {"tag_name": "v0.5.0", "draft": true, "published_at": null, "body": ""},
          {"tag_name": "v0.4.2", "published_at": "2026-10-02T10:00:00Z", "body": "## Sideporch 0.4.2\n\n### Security\n\n- Fix", "html_url": "https://github.com/niklas-heer/sideporch/releases/tag/v0.4.2"},
          {"tag_name": "v0.4.1", "published_at": "2026-09-29T13:13:30Z", "body": "### Documentation\n\n- Security of the docs", "html_url": "u"},
          {"tag_name": "nightly", "published_at": "2026-09-29T13:13:30Z", "body": ""}
        ]"####;
        let releases = parse_releases(json).unwrap();
        let versions: Vec<String> = releases.iter().map(|r| r.version.to_string()).collect();
        assert_eq!(versions, ["0.4.2", "0.4.1"]);
        assert!(
            releases[0].security,
            "a Security section marks a security release"
        );
        assert!(!releases[1].security, "the word alone doesn't");
        assert_eq!(
            releases[0].url,
            "https://github.com/niklas-heer/sideporch/releases/tag/v0.4.2"
        );
        assert!(parse_releases("{\"message\": \"rate limited\"}").is_err());
    }

    #[test]
    fn urgency_grows_the_longer_an_update_waits() {
        let now = 100 * DAY;
        assert_eq!(Urgency::of(&[], now), None);
        let fresh = [release("0.5.0", now - 3 * DAY, false)];
        assert_eq!(Urgency::of(&fresh, now), Some(Urgency::Available));
        // Counted from the first release the server is missing.
        let waiting = [
            release("0.5.1", now - DAY, false),
            release("0.5.0", now - 20 * DAY, false),
        ];
        assert_eq!(Urgency::of(&waiting, now), Some(Urgency::Remind));
        let old = [release("0.5.0", now - 61 * DAY, false)];
        assert_eq!(Urgency::of(&old, now), Some(Urgency::Overdue));
        let security = [
            release("0.5.1", now, true),
            release("0.5.0", now - 61 * DAY, false),
        ];
        assert_eq!(Urgency::of(&security, now), Some(Urgency::Security));
        assert_eq!(Urgency::Available.hide_for_days(), None);
        assert_eq!(Urgency::Remind.hide_for_days(), Some(7));
        assert_eq!(Urgency::Security.hide_for_days(), Some(1));
    }

    #[test]
    fn newer_releases_only() {
        let all = [
            release("0.5.0", 0, false),
            release("0.4.1", 0, false),
            release("0.4.0", 0, false),
        ];
        let newer = newer_than(&all, Version(0, 4, 1));
        assert_eq!(newer.len(), 1);
        assert_eq!(newer[0].version, Version(0, 5, 0));
    }
}
