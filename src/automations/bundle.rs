//! Automations as a file to share: one or several automations with the
//! libraries they need, exported from one server and imported on another.
//!
//! The file is versioned JSON. Scripts travel as source only, with the
//! names of the secrets they read and the libraries they load; secret
//! values, stored data, run logs, webhook URLs and history never leave the
//! server. Readers ignore fields they don't know, so later versions, or an
//! automation store serving the same files, can add descriptive fields.

use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

use super::{KIND_AUTOMATION, KIND_LIBRARY, MAX_SOURCE_BYTES, tooling, valid_library_name};
use crate::store::Automation;

pub const FORMAT: &str = "sideporch-automations";
pub const VERSION: u32 = 1;
/// The most automations and libraries one file may hold.
pub const MAX_ITEMS: usize = 100;
/// The largest file Sideporch reads, in bytes.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub format: String,
    pub version: u32,
    /// The Sideporch version that wrote the file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub sideporch: String,
    /// What the collection is for, when someone describes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub homepage: Option<String>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    /// `automation`, or `library` for code automations `require`.
    pub kind: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: String,
    /// Secrets the script reads with `sideporch.secret`.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// Libraries the script loads with `require`.
    #[serde(default)]
    pub requires: Vec<String>,
}

impl Item {
    fn new(automation: &Automation) -> Self {
        Self {
            kind: automation.kind.clone(),
            name: automation.name.clone(),
            description: None,
            source: automation.source.clone(),
            secrets: secrets_used(&automation.source),
            requires: libraries_required(&automation.source),
        }
    }

    pub fn is_library(&self) -> bool {
        self.kind == KIND_LIBRARY
    }
}

/// The string literals passed to `call`, as in `call("x")`, `call('x')` or
/// `call "x"`. Good enough to name what a script needs; a call hidden in
/// a comment is named too, which does no harm.
fn literal_arguments(source: &str, call: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    for (at, _) in source.match_indices(call) {
        let before = source.get(..at).and_then(|text| text.chars().next_back());
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let rest = source
            .get(at.saturating_add(call.len())..)
            .unwrap_or_default()
            .trim_start();
        let rest = rest.strip_prefix('(').map_or(rest, str::trim_start);
        let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            continue;
        };
        let body = rest.get(1..).unwrap_or_default();
        if let Some(end) = body.find(quote) {
            let value = body.get(..end).unwrap_or_default();
            if !value.is_empty() && !value.contains(['\n', '\\']) {
                found.insert(value.to_owned());
            }
        }
    }
    found.into_iter().collect()
}

/// Names of the secrets `source` reads.
pub fn secrets_used(source: &str) -> Vec<String> {
    literal_arguments(source, "sideporch.secret")
}

/// Names of the libraries `source` loads.
pub fn libraries_required(source: &str) -> Vec<String> {
    literal_arguments(source, "require")
}

/// A file with the chosen automations, or everything when `chosen` is
/// empty, plus every library they need. Libraries come first.
pub fn export(all: &[Automation], chosen: &[i64]) -> Bundle {
    let picked: Vec<&Automation> = if chosen.is_empty() {
        all.iter().collect()
    } else {
        all.iter()
            .filter(|automation| chosen.contains(&automation.id))
            .collect()
    };
    let mut libraries: BTreeSet<&str> = picked
        .iter()
        .filter(|automation| automation.kind == KIND_LIBRARY)
        .map(|automation| automation.name.as_str())
        .collect();
    // Libraries can need libraries too.
    let mut pending: Vec<String> = picked
        .iter()
        .flat_map(|automation| libraries_required(&automation.source))
        .collect();
    while let Some(name) = pending.pop() {
        if let Some(library) = all
            .iter()
            .find(|automation| automation.kind == KIND_LIBRARY && automation.name == name)
            && libraries.insert(library.name.as_str())
        {
            pending.extend(libraries_required(&library.source));
        }
    }
    let mut items: Vec<Item> = all
        .iter()
        .filter(|automation| {
            automation.kind == KIND_LIBRARY && libraries.contains(automation.name.as_str())
        })
        .map(Item::new)
        .collect();
    items.extend(
        picked
            .iter()
            .filter(|automation| automation.kind != KIND_LIBRARY)
            .map(|automation| Item::new(automation)),
    );
    Bundle {
        format: FORMAT.to_owned(),
        version: VERSION,
        sideporch: env!("CARGO_PKG_VERSION").to_owned(),
        description: None,
        author: None,
        license: None,
        homepage: None,
        items,
    }
}

const NOT_A_BUNDLE: &str =
    "This isn't a Sideporch automation file. Export one from Automations on another server.";

/// Reads a file someone wants to import, or says why it can't be.
pub fn parse(text: &str) -> Result<Bundle, String> {
    if text.len() > MAX_BYTES {
        return Err("The file is larger than 2 MB.".to_owned());
    }
    let mut bundle: Bundle = serde_json::from_str(text).map_err(|_| NOT_A_BUNDLE.to_owned())?;
    if bundle.format != FORMAT || bundle.version == 0 {
        return Err(NOT_A_BUNDLE.to_owned());
    }
    if bundle.version > VERSION {
        return Err(format!(
            "This file was made by a newer Sideporch (format version {}). Update Sideporch to import it.",
            bundle.version
        ));
    }
    if bundle.items.is_empty() {
        return Err("The file holds no automations.".to_owned());
    }
    if bundle.items.len() > MAX_ITEMS {
        return Err(format!("The file holds more than {MAX_ITEMS} automations."));
    }
    let mut libraries = HashSet::new();
    for item in &mut bundle.items {
        if item.kind != KIND_AUTOMATION && item.kind != KIND_LIBRARY {
            return Err(format!(
                "{} is a kind of script this Sideporch doesn't know ({}).",
                item.name, item.kind
            ));
        }
        item.name = item.name.trim().chars().take(80).collect();
        if item.name.is_empty() {
            return Err("Every automation in the file needs a name.".to_owned());
        }
        if item.source.len() > MAX_SOURCE_BYTES {
            return Err(format!("{} is longer than 100 kB.", item.name));
        }
        if item.is_library() {
            if !valid_library_name(&item.name) {
                return Err(format!("{} isn't a valid library name.", item.name));
            }
            if !libraries.insert(item.name.clone()) {
                return Err(format!("The library {} is in the file twice.", item.name));
            }
        }
        // What a script needs is read from the script, not taken on trust.
        item.secrets = secrets_used(&item.source);
        item.requires = libraries_required(&item.source);
    }
    // Libraries first, so automations find them when they start.
    bundle.items.sort_by_key(|item| !item.is_library());
    Ok(bundle)
}

/// What importing an item would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Add it; for items whose name is free.
    Import,
    /// Overwrite the existing one of the same name.
    Replace,
    /// Add it under a new name next to the existing one; automations only,
    /// since a library is found by its name.
    Copy,
    Skip,
}

impl Action {
    pub const fn key(self) -> &'static str {
        match self {
            Self::Import => "import",
            Self::Replace => "replace",
            Self::Copy => "copy",
            Self::Skip => "skip",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        [Self::Import, Self::Replace, Self::Copy, Self::Skip]
            .into_iter()
            .find(|action| action.key() == key)
    }
}

/// One item of an import, checked against this server.
#[derive(Debug, Clone)]
pub struct Plan {
    pub item: Item,
    /// The automation or library of the same kind and name here.
    pub existing: Option<i64>,
    /// The existing one has the same script.
    pub unchanged: bool,
    /// Secrets it reads that this server doesn't have yet.
    pub missing_secrets: Vec<String>,
    /// Libraries it loads that are neither here nor in the file.
    pub missing_libraries: Vec<String>,
    pub errors: usize,
    pub warnings: usize,
}

impl Plan {
    /// What to offer, the suggestion first.
    pub fn choices(&self) -> Vec<Action> {
        match (self.existing, self.item.is_library(), self.unchanged) {
            (None, _, _) => vec![Action::Import, Action::Skip],
            (Some(_), true, _) | (Some(_), _, true) => vec![Action::Skip, Action::Replace],
            (Some(_), false, false) => vec![Action::Copy, Action::Replace, Action::Skip],
        }
    }

    /// Whether `action` may be taken for this item.
    pub fn allows(&self, action: Action) -> bool {
        self.choices().contains(&action)
    }
}

/// Checks each item of `bundle` against the automations and secrets here.
pub fn plan(bundle: &Bundle, here: &[Automation], secrets: &BTreeSet<String>) -> Vec<Plan> {
    let libraries: BTreeSet<&str> = here
        .iter()
        .filter(|automation| automation.kind == KIND_LIBRARY)
        .map(|automation| automation.name.as_str())
        .chain(
            bundle
                .items
                .iter()
                .filter(|item| item.is_library())
                .map(|item| item.name.as_str()),
        )
        .collect();
    bundle
        .items
        .iter()
        .map(|item| {
            let existing = here
                .iter()
                .find(|automation| automation.kind == item.kind && automation.name == item.name);
            let problems = tooling::lint(&item.source);
            let errors = problems
                .iter()
                .filter(|problem| problem.severity == tooling::Severity::Error)
                .count();
            Plan {
                existing: existing.map(|automation| automation.id),
                unchanged: existing.is_some_and(|automation| automation.source == item.source),
                missing_secrets: item
                    .secrets
                    .iter()
                    .filter(|name| !secrets.contains(*name))
                    .cloned()
                    .collect(),
                missing_libraries: item
                    .requires
                    .iter()
                    .filter(|name| !libraries.contains(name.as_str()))
                    .cloned()
                    .collect(),
                errors,
                warnings: problems.len().saturating_sub(errors),
                item: item.clone(),
            }
        })
        .collect()
}

/// A name for a copy of `name` that no automation here has.
pub fn copy_name(name: &str, here: &[Automation]) -> String {
    let taken = |candidate: &str| {
        here.iter()
            .any(|automation| automation.kind == KIND_AUTOMATION && automation.name == candidate)
    };
    let mut candidate = format!("{name} (imported)");
    let mut number = 2_u32;
    while taken(&candidate) {
        candidate = format!("{name} (imported {number})");
        number = number.saturating_add(1);
    }
    candidate
}

/// A file name for a bundle of `items`, like `weather.sideporch.json`.
pub fn file_name(items: &[Item]) -> String {
    let mut automations = items.iter().filter(|item| !item.is_library());
    let stem = match (automations.next(), automations.next()) {
        (Some(only), None) => {
            let slug: String = only
                .name
                .to_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                .collect();
            let slug = slug
                .split('-')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("-");
            if slug.is_empty() {
                "automation".to_owned()
            } else {
                slug
            }
        }
        _ => "automations".to_owned(),
    };
    format!("{stem}.sideporch.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn automation(id: i64, kind: &str, name: &str, source: &str) -> Automation {
        Automation {
            id,
            name: name.to_owned(),
            source: source.to_owned(),
            enabled: true,
            last_error: None,
            updated_at: 0,
            hook_token: "secret-token".to_owned(),
            kind: kind.to_owned(),
        }
    }

    #[test]
    fn finds_what_a_script_needs() {
        let source = r#"
            local api = require("github_api")
            local util = require 'util'
            local token = sideporch.secret("GITHUB_TOKEN")
            local other = sideporch.secret 'SLACK'
            local dynamic = sideporch.secret(name)
            local not_it = my_require("x")
        "#;
        assert_eq!(secrets_used(source), ["GITHUB_TOKEN", "SLACK"]);
        assert_eq!(libraries_required(source), ["github_api", "util"]);
    }

    #[test]
    fn exports_the_libraries_a_script_needs_even_through_other_libraries() {
        let all = [
            automation(1, KIND_LIBRARY, "http_json", "return {}"),
            automation(
                2,
                KIND_LIBRARY,
                "github_api",
                "local h = require(\"http_json\")",
            ),
            automation(3, KIND_LIBRARY, "unused", "return {}"),
            automation(
                4,
                KIND_AUTOMATION,
                "Deploys",
                "local gh = require(\"github_api\")",
            ),
            automation(5, KIND_AUTOMATION, "Welcome", "print(1)"),
        ];
        let bundle = export(&all, &[4]);
        let names: Vec<&str> = bundle.items.iter().map(|item| item.name.as_str()).collect();
        assert_eq!(names, ["http_json", "github_api", "Deploys"]);
        assert_eq!(export(&all, &[]).items.len(), 5);
        let text = serde_json::to_string(&bundle).unwrap_or_default();
        assert!(!text.contains("secret-token"));
        assert_eq!(file_name(&bundle.items), "deploys.sideporch.json");
    }

    #[test]
    fn reads_files_from_later_versions_it_understands_and_refuses_the_rest() {
        let file = r#"{"format":"sideporch-automations","version":1,"store_id":"abc",
            "items":[{"kind":"automation","name":"A","source":"sideporch.secret('X')","rating":5},
                     {"kind":"library","name":"lib","source":"return {}"}]}"#;
        let bundle = parse(file).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            bundle.items.first().map(|item| item.name.as_str()),
            Some("lib")
        );
        assert_eq!(
            bundle.items.get(1).map(|item| item.secrets.clone()),
            Some(vec!["X".to_owned()])
        );
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"format":"sideporch-automations","version":1,"items":[{"kind":"library","name":"Bad Name","source":""}]}"#).is_err());
    }

    #[test]
    fn copies_get_a_free_name() {
        let here = [
            automation(1, KIND_AUTOMATION, "Weather", ""),
            automation(2, KIND_AUTOMATION, "Weather (imported)", ""),
        ];
        assert_eq!(copy_name("Weather", &here), "Weather (imported 2)");
    }
}
