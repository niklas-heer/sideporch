//! Settings an automation declares in its file, so people can change
//! them on the automation's page instead of editing the script. The script
//! reads them with `sideporch.setting("name")`.
//!
//! A declaration names a setting, its type (`string`, `number`, `boolean`
//! or `channel`, which holds the name of a public channel), an optional
//! label and default, and whether it is required. Values are stored per
//! automation, as text, and checked against the declaration when they are
//! saved.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most settings one automation may declare.
pub const MAX_SETTINGS: usize = 20;
/// The longest value a setting can hold, in characters.
pub const MAX_VALUE_CHARS: usize = 1000;
/// The types a setting can have.
pub const KINDS: [&str; 4] = ["string", "number", "boolean", "channel"];

/// One setting an automation declares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Declaration {
    pub name: String,
    /// What the editor and the preview call the setting; the name if none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// `string`, `number`, `boolean` or `channel`.
    #[serde(rename = "type")]
    pub kind: String,
    /// The value used until someone sets one, of the declared type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Whether the automation can't be switched on without a value.
    #[serde(default)]
    pub required: bool,
}

/// What a script receives from `sideporch.setting`.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Text(String),
    Number(f64),
    Boolean(bool),
}

/// Names a setting can have, as `sideporch.setting` takes them.
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && name.len() <= 40
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Checks what an automation declares, or says what is wrong with it.
pub fn validate(declarations: &[Declaration]) -> Result<(), String> {
    if declarations.len() > MAX_SETTINGS {
        return Err(format!("it declares more than {MAX_SETTINGS} settings"));
    }
    let mut names = std::collections::HashSet::new();
    for declaration in declarations {
        if !valid_name(&declaration.name) {
            return Err(format!(
                "the setting {} needs a name like `greeting`: lowercase letters, digits and underscores, starting with a letter",
                declaration.name
            ));
        }
        if !names.insert(declaration.name.as_str()) {
            return Err(format!(
                "the setting {} is declared twice",
                declaration.name
            ));
        }
        if !KINDS.contains(&declaration.kind.as_str()) {
            return Err(format!(
                "the setting {} has a type this Sideporch doesn't know ({})",
                declaration.name, declaration.kind
            ));
        }
        if let Some(label) = &declaration.label
            && label.chars().count() > 80
        {
            return Err(format!(
                "the label of {} is longer than 80 characters",
                declaration.name
            ));
        }
        if let Some(default) = &declaration.default {
            let fits = match declaration.kind.as_str() {
                "number" => default.as_f64().is_some_and(f64::is_finite),
                "boolean" => default.is_boolean(),
                _ => default.is_string(),
            };
            if !fits {
                return Err(format!(
                    "the default of {} isn't a {}",
                    declaration.name, declaration.kind
                ));
            }
        }
    }
    Ok(())
}

/// What a setting is called on the page.
pub fn label(declaration: &Declaration) -> &str {
    declaration.label.as_deref().unwrap_or(&declaration.name)
}

/// The default as text, the way values are stored.
pub fn default_text(declaration: &Declaration) -> Option<String> {
    match declaration.default.as_ref()? {
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// The text of a value from the page, checked against the declaration and
/// written the way it is stored. A channel is named without `#`; whether
/// it is a public channel is for the caller to check.
pub fn check_value(declaration: &Declaration, text: &str) -> Result<String, String> {
    let text = text.trim();
    let label = label(declaration);
    match declaration.kind.as_str() {
        "number" => text
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .map(|number| number.to_string())
            .ok_or_else(|| format!("{label} needs a number.")),
        "boolean" => match text {
            "true" | "false" => Ok(text.to_owned()),
            _ => Err(format!("{label} must be true or false.")),
        },
        "channel" => {
            let name = text.trim_start_matches('#');
            if name.is_empty() {
                Err(format!("{label} needs a channel name."))
            } else {
                Ok(name.to_owned())
            }
        }
        _ => {
            if text.chars().count() > MAX_VALUE_CHARS {
                Err(format!(
                    "{label} is longer than {MAX_VALUE_CHARS} characters."
                ))
            } else {
                Ok(text.to_owned())
            }
        }
    }
}

/// The value each setting has now: what was saved, else the default.
pub fn resolve(
    declarations: &[Declaration],
    saved: &HashMap<String, String>,
) -> BTreeMap<String, SettingValue> {
    declarations
        .iter()
        .filter_map(|declaration| {
            let text = saved
                .get(&declaration.name)
                .cloned()
                .or_else(|| default_text(declaration))?;
            let value = match declaration.kind.as_str() {
                "number" => SettingValue::Number(text.parse().ok()?),
                "boolean" => SettingValue::Boolean(text == "true"),
                _ => SettingValue::Text(text),
            };
            Some((declaration.name.clone(), value))
        })
        .collect()
}

/// The labels of the required settings that have no value and no default.
pub fn missing_required(
    declarations: &[Declaration],
    saved: &HashMap<String, String>,
) -> Vec<String> {
    declarations
        .iter()
        .filter(|declaration| {
            declaration.required
                && !saved.contains_key(&declaration.name)
                && default_text(declaration).is_none()
        })
        .map(|declaration| label(declaration).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn declaration(value: Value) -> Declaration {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn declarations_are_checked() {
        let good = [
            declaration(json!({"name": "greeting", "type": "string", "default": "hi"})),
            declaration(json!({"name": "limit", "type": "number", "default": 3})),
            declaration(json!({"name": "loud", "type": "boolean", "default": false})),
            declaration(json!({"name": "room", "type": "channel", "required": true})),
        ];
        assert_eq!(validate(&good), Ok(()));
        let bad_name = [declaration(json!({"name": "Bad Name", "type": "string"}))];
        assert!(validate(&bad_name).is_err());
        let unknown_type = [declaration(json!({"name": "x", "type": "date"}))];
        assert!(validate(&unknown_type).is_err());
        let wrong_default = [declaration(
            json!({"name": "x", "type": "number", "default": "3"}),
        )];
        assert!(validate(&wrong_default).is_err());
        let twice = [good[0].clone(), good[0].clone()];
        assert!(validate(&twice).is_err());
    }

    #[test]
    fn values_are_checked_by_type() {
        let number = declaration(json!({"name": "limit", "type": "number"}));
        assert_eq!(check_value(&number, " 2.5 "), Ok("2.5".to_owned()));
        assert!(check_value(&number, "many").is_err());
        assert!(check_value(&number, "inf").is_err());
        let channel = declaration(json!({"name": "room", "type": "channel"}));
        assert_eq!(check_value(&channel, "#news"), Ok("news".to_owned()));
        assert!(check_value(&channel, "#").is_err());
        let flag = declaration(json!({"name": "loud", "type": "boolean"}));
        assert_eq!(check_value(&flag, "true"), Ok("true".to_owned()));
        assert!(check_value(&flag, "yes").is_err());
    }

    #[test]
    fn saved_values_win_over_defaults_and_required_ones_are_named() {
        let declarations = [
            declaration(json!({"name": "greeting", "type": "string", "default": "hi"})),
            declaration(json!({"name": "limit", "type": "number", "default": 3})),
            declaration(
                json!({"name": "room", "type": "channel", "label": "Room", "required": true}),
            ),
        ];
        let saved = HashMap::from([("limit".to_owned(), "7".to_owned())]);
        let values = resolve(&declarations, &saved);
        assert_eq!(
            values.get("greeting"),
            Some(&SettingValue::Text("hi".to_owned()))
        );
        assert_eq!(values.get("limit"), Some(&SettingValue::Number(7.0)));
        assert!(!values.contains_key("room"));
        assert_eq!(missing_required(&declarations, &saved), ["Room"]);
        let saved = HashMap::from([("room".to_owned(), "news".to_owned())]);
        assert!(missing_required(&declarations, &saved).is_empty());
    }
}
