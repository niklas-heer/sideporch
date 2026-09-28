//! Colour themes: popular editor and terminal palettes, mapped onto
//! Sideporch's colour tokens.
//!
//! The stylesheet refers to tokens (`--floor`, `--night` and so on) instead
//! of fixed colours, and each theme sets them for light and dark mode. People
//! pick a theme and whether it follows their system's light or dark mode or
//! stays on one; admins pick the default. Themes that only exist in one mode
//! stay in it.

use std::{fmt::Write as _, sync::LazyLock};

use rusqlite::Connection;

use crate::{
    error::{AppError, AppResult},
    store,
};

/// Values for every colour token.
///
/// - `floor`, `floor_2`, `floor_3`: the sidebar, its hover and its selected
///   item (with light text on all three), and primary buttons in light mode.
///   `floor_3` is also the accent for links and focus.
/// - `haint`, `haint_2`: light accents: secondary and primary text on the
///   sidebar and in dark mode, and highlights in light mode.
/// - `lamp`: unread dots and markers.
/// - `ink`, `muted`, `line`, `screen`, `white`: light mode's text, secondary
///   text, borders, subtle surfaces and background.
/// - `night`, `night_2`, `night_line`: dark mode's background, surfaces and
///   borders.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub floor: &'static str,
    pub floor_2: &'static str,
    pub floor_3: &'static str,
    pub haint: &'static str,
    pub haint_2: &'static str,
    pub lamp: &'static str,
    pub ink: &'static str,
    pub muted: &'static str,
    pub line: &'static str,
    pub screen: &'static str,
    pub white: &'static str,
    pub night: &'static str,
    pub night_2: &'static str,
    pub night_line: &'static str,
}

impl Palette {
    const fn tokens(&self) -> [(&'static str, &'static str); 14] {
        [
            ("floor", self.floor),
            ("floor-2", self.floor_2),
            ("floor-3", self.floor_3),
            ("haint", self.haint),
            ("haint-2", self.haint_2),
            ("lamp", self.lamp),
            ("ink", self.ink),
            ("muted", self.muted),
            ("line", self.line),
            ("screen", self.screen),
            ("white", self.white),
            ("night", self.night),
            ("night-2", self.night_2),
            ("night-line", self.night_line),
        ]
    }
}

#[derive(Debug)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub light: Option<Palette>,
    pub dark: Option<Palette>,
}

/// Light and dark palettes that share the light-mode tokens.
macro_rules! palette {
    ($floor:literal, $floor_2:literal, $floor_3:literal, $haint:literal, $haint_2:literal, $lamp:literal,
     $ink:literal, $muted:literal, $line:literal, $screen:literal, $white:literal,
     $night:literal, $night_2:literal, $night_line:literal) => {
        Palette {
            floor: $floor,
            floor_2: $floor_2,
            floor_3: $floor_3,
            haint: $haint,
            haint_2: $haint_2,
            lamp: $lamp,
            ink: $ink,
            muted: $muted,
            line: $line,
            screen: $screen,
            white: $white,
            night: $night,
            night_2: $night_2,
            night_line: $night_line,
        }
    };
}

pub const DEFAULT_THEME: &str = "sideporch";

/// Sideporch's own palette, the default.
const SIDEPORCH: Theme = Theme {
    id: "sideporch",
    name: "Sideporch",
    light: Some(palette!(
        "#24403C", "#2E524D", "#3B6660", "#B9E0DA", "#DCEFEC", "#F5C04A", "#1B2826", "#5A6D69",
        "#DCE4E2", "#F5F8F7", "#FFFFFF", "#101A19", "#172422", "#2A3B38"
    )),
    dark: Some(palette!(
        "#24403C", "#2E524D", "#3B6660", "#B9E0DA", "#DCEFEC", "#F5C04A", "#1B2826", "#5A6D69",
        "#DCE4E2", "#F5F8F7", "#FFFFFF", "#101A19", "#172422", "#2A3B38"
    )),
};

pub const THEMES: &[Theme] = &[
    SIDEPORCH,
    Theme {
        id: "github",
        name: "GitHub",
        light: Some(palette!(
            "#24292F", "#32383F", "#0969DA", "#C9D1D9", "#DDF4FF", "#D29922", "#1F2328", "#59636E",
            "#D1D9E0", "#F6F8FA", "#FFFFFF", "#0D1117", "#151B23", "#3D444D"
        )),
        dark: Some(palette!(
            "#010409", "#151B23", "#1F6FEB", "#9198A1", "#F0F6FC", "#D29922", "#1F2328", "#59636E",
            "#D1D9E0", "#F6F8FA", "#F0F6FC", "#0D1117", "#151B23", "#3D444D"
        )),
    },
    Theme {
        id: "aubergine",
        name: "Aubergine",
        light: Some(palette!(
            "#3F0E40", "#521653", "#1164A3", "#CFC3CF", "#E8F5FA", "#CD2553", "#1D1C1D", "#616061",
            "#DDDDDD", "#F8F8F8", "#FFFFFF", "#1A1D21", "#222529", "#35373B"
        )),
        dark: Some(palette!(
            "#19171D", "#27242C", "#1164A3", "#ABABAD", "#D1D2D3", "#CD2553", "#1D1C1D", "#616061",
            "#DDDDDD", "#F8F8F8", "#D1D2D3", "#1A1D21", "#222529", "#35373B"
        )),
    },
    Theme {
        id: "solarized",
        name: "Solarized",
        light: Some(palette!(
            "#073642", "#0B4452", "#268BD2", "#93A1A1", "#EEE8D5", "#B58900", "#073642", "#586E75",
            "#E4DDC8", "#EEE8D5", "#FDF6E3", "#002B36", "#073642", "#0E4B59"
        )),
        dark: Some(palette!(
            "#00212B", "#073642", "#268BD2", "#93A1A1", "#EEE8D5", "#B58900", "#073642", "#586E75",
            "#E4DDC8", "#EEE8D5", "#FDF6E3", "#002B36", "#073642", "#0E4B59"
        )),
    },
    Theme {
        id: "gruvbox",
        name: "Gruvbox",
        light: Some(palette!(
            "#3C3836", "#504945", "#458588", "#D5C4A1", "#EBDBB2", "#D79921", "#3C3836", "#7C6F64",
            "#D5C4A1", "#F2E5BC", "#FBF1C7", "#282828", "#32302F", "#504945"
        )),
        dark: Some(palette!(
            "#1D2021", "#32302F", "#458588", "#A89984", "#EBDBB2", "#FABD2F", "#3C3836", "#7C6F64",
            "#D5C4A1", "#F2E5BC", "#FBF1C7", "#282828", "#32302F", "#504945"
        )),
    },
    Theme {
        id: "catppuccin",
        name: "Catppuccin",
        light: Some(palette!(
            "#1E1E2E", "#313244", "#8839EF", "#BAC2DE", "#E6E9EF", "#FE640B", "#4C4F69", "#6C6F85",
            "#CCD0DA", "#E6E9EF", "#EFF1F5", "#1E1E2E", "#313244", "#45475A"
        )),
        dark: Some(palette!(
            "#11111B", "#181825", "#45475A", "#A6ADC8", "#CDD6F4", "#F9E2AF", "#4C4F69", "#6C6F85",
            "#CCD0DA", "#E6E9EF", "#CDD6F4", "#1E1E2E", "#313244", "#45475A"
        )),
    },
    Theme {
        id: "rose-pine",
        name: "Rosé Pine",
        light: Some(palette!(
            "#26233A", "#393552", "#907AA9", "#908CAA", "#F2E9E1", "#EA9D34", "#575279", "#797593",
            "#DFDAD9", "#FFFAF3", "#FAF4ED", "#191724", "#1F1D2E", "#403D52"
        )),
        dark: Some(palette!(
            "#1F1D2E", "#26233A", "#524F67", "#908CAA", "#E0DEF4", "#F6C177", "#575279", "#797593",
            "#DFDAD9", "#FFFAF3", "#E0DEF4", "#191724", "#1F1D2E", "#403D52"
        )),
    },
    Theme {
        id: "nord",
        name: "Nord",
        light: Some(palette!(
            "#2E3440", "#3B4252", "#5E81AC", "#D8DEE9", "#E5E9F0", "#D08770", "#2E3440", "#4C566A",
            "#D8DEE9", "#E5E9F0", "#ECEFF4", "#2E3440", "#3B4252", "#434C5E"
        )),
        dark: Some(palette!(
            "#242933", "#3B4252", "#5E81AC", "#D8DEE9", "#ECEFF4", "#EBCB8B", "#2E3440", "#4C566A",
            "#D8DEE9", "#E5E9F0", "#ECEFF4", "#2E3440", "#3B4252", "#434C5E"
        )),
    },
    Theme {
        id: "one",
        name: "One",
        light: Some(palette!(
            "#282C34", "#2C313A", "#4078F2", "#ABB2BF", "#E5E5E6", "#C18401", "#383A42", "#696C77",
            "#DBDBDC", "#F0F0F1", "#FAFAFA", "#282C34", "#2C313A", "#3E4451"
        )),
        dark: Some(palette!(
            "#21252B", "#2C313A", "#3E4451", "#ABB2BF", "#D7DAE0", "#E5C07B", "#383A42", "#696C77",
            "#DBDBDC", "#F0F0F1", "#D7DAE0", "#282C34", "#2C313A", "#3E4451"
        )),
    },
    Theme {
        id: "everforest",
        name: "Everforest",
        light: Some(palette!(
            "#2D353B", "#343F44", "#35A77C", "#D3C6AA", "#EFEBD4", "#DFA000", "#5C6A72", "#829181",
            "#E0DCC7", "#F4F0D9", "#FDF6E3", "#2D353B", "#343F44", "#475258"
        )),
        dark: Some(palette!(
            "#232A2E", "#343F44", "#4F5B58", "#9DA9A0", "#D3C6AA", "#DBBC7F", "#5C6A72", "#829181",
            "#E0DCC7", "#F4F0D9", "#D3C6AA", "#2D353B", "#343F44", "#475258"
        )),
    },
    Theme {
        id: "tokyo-night",
        name: "Tokyo Night",
        light: Some(palette!(
            "#1A1B26", "#24283B", "#2E7DE9", "#A9B1D6", "#E1E2E7", "#8C6C3E", "#3760BF", "#6172B0",
            "#C4C8DA", "#E9E9ED", "#F4F4F7", "#1A1B26", "#24283B", "#414868"
        )),
        dark: Some(palette!(
            "#16161E", "#1F2335", "#3D59A1", "#A9B1D6", "#C0CAF5", "#E0AF68", "#3760BF", "#6172B0",
            "#C4C8DA", "#E9E9ED", "#C0CAF5", "#1A1B26", "#24283B", "#414868"
        )),
    },
    Theme {
        id: "ayu",
        name: "Ayu",
        light: Some(palette!(
            "#1F2430", "#242936", "#E07A2E", "#CCCAC2", "#F3F4F5", "#F2AE49", "#5C6166", "#787B80",
            "#E7E8E9", "#F3F4F5", "#FCFCFC", "#1F2430", "#242936", "#33415E"
        )),
        dark: Some(palette!(
            "#1A1F29", "#242936", "#33415E", "#8A9199", "#CCCAC2", "#FFCC66", "#5C6166", "#787B80",
            "#E7E8E9", "#F3F4F5", "#CCCAC2", "#1F2430", "#242936", "#33415E"
        )),
    },
    Theme {
        id: "tomorrow",
        name: "Tomorrow",
        light: Some(palette!(
            "#1D1F21", "#282A2E", "#4271AE", "#C5C8C6", "#EFEFEF", "#EAB700", "#4D4D4C", "#6E706C",
            "#D6D6D6", "#EFEFEF", "#FFFFFF", "#1D1F21", "#282A2E", "#373B41"
        )),
        dark: Some(palette!(
            "#161719", "#282A2E", "#4271AE", "#B4B7B4", "#C5C8C6", "#F0C674", "#4D4D4C", "#6E706C",
            "#D6D6D6", "#EFEFEF", "#C5C8C6", "#1D1F21", "#282A2E", "#373B41"
        )),
    },
    Theme {
        id: "material",
        name: "Material",
        light: Some(palette!(
            "#263238", "#2E3C43", "#6182B8", "#B0BEC5", "#ECEFF1", "#F6A434", "#37474F", "#546E7A",
            "#E7EAEC", "#F5F5F5", "#FAFAFA", "#292D3E", "#32374D", "#4E5579"
        )),
        dark: Some(palette!(
            "#202331", "#292D3E", "#7E57C2", "#A6ACCD", "#D0D2E8", "#FFCB6B", "#37474F", "#546E7A",
            "#E7EAEC", "#F5F5F5", "#EEFFFF", "#292D3E", "#32374D", "#4E5579"
        )),
    },
    Theme {
        id: "high-contrast",
        name: "High contrast",
        light: Some(palette!(
            "#000000", "#1A1A1A", "#0000CC", "#FFFFFF", "#FFFFFF", "#FFD700", "#000000", "#333333",
            "#595959", "#F0F0F0", "#FFFFFF", "#000000", "#121212", "#8A8A8A"
        )),
        dark: Some(palette!(
            "#000000", "#1A1A1A", "#3333FF", "#FFFFFF", "#FFFFFF", "#FFD700", "#000000", "#333333",
            "#595959", "#F0F0F0", "#FFFFFF", "#000000", "#121212", "#8A8A8A"
        )),
    },
    Theme {
        id: "paper",
        name: "Paper",
        light: Some(palette!(
            "#3B342C", "#4A4238", "#8B5E34", "#D9CDB8", "#F1E9DA", "#C8872B", "#2B2620", "#6E6456",
            "#E3D9C6", "#F3ECDF", "#FBF7EF", "#221E19", "#2C2721", "#463E33"
        )),
        dark: None,
    },
    Theme {
        id: "dracula",
        name: "Dracula",
        light: None,
        dark: Some(palette!(
            "#21222C", "#343746", "#6272A4", "#BD93F9", "#F8F8F2", "#FF79C6", "#282A36", "#6272A4",
            "#44475A", "#313341", "#F8F8F2", "#282A36", "#313341", "#44475A"
        )),
    },
    Theme {
        id: "monokai",
        name: "Monokai",
        light: None,
        dark: Some(palette!(
            "#1E1F1C", "#3E3D32", "#75715E", "#CFCFC2", "#F8F8F2", "#E6DB74", "#272822", "#75715E",
            "#49483E", "#3E3D32", "#F8F8F2", "#272822", "#2F302A", "#49483E"
        )),
    },
    Theme {
        id: "kanagawa",
        name: "Kanagawa",
        light: None,
        dark: Some(palette!(
            "#16161D", "#2A2A37", "#2D4F67", "#C8C093", "#DCD7BA", "#E6C384", "#1F1F28", "#727169",
            "#363646", "#2A2A37", "#DCD7BA", "#1F1F28", "#2A2A37", "#363646"
        )),
    },
    Theme {
        id: "night-owl",
        name: "Night Owl",
        light: None,
        dark: Some(palette!(
            "#010E1A", "#0B2942", "#1D3B53", "#82AAFF", "#D6DEEB", "#ECC48D", "#011627", "#5F7E97",
            "#1D3B53", "#0B2942", "#D6DEEB", "#011627", "#0B2942", "#1D3B53"
        )),
    },
];

pub fn find(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|theme| theme.id == id)
}

/// Whether a theme follows the system's light or dark mode, or stays on one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// A theme and appearance, as a page applies them.
#[derive(Debug, Clone, Copy)]
pub struct Choice {
    pub theme: &'static Theme,
    pub appearance: Appearance,
}

impl Default for Choice {
    fn default() -> Self {
        Self::resolve("", "", DEFAULT_THEME, "system")
    }
}

impl Choice {
    /// A person's choice, falling back to the instance's defaults, and
    /// kept to the modes the theme has.
    pub fn resolve(
        theme: &str,
        appearance: &str,
        default_theme: &str,
        default_appearance: &str,
    ) -> Self {
        let chosen = find(theme)
            .or_else(|| find(default_theme))
            .unwrap_or(&SIDEPORCH);
        let appearance = Appearance::parse(appearance)
            .or_else(|| Appearance::parse(default_appearance))
            .unwrap_or_default();
        let appearance = match (chosen.light, chosen.dark) {
            (None, _) => Appearance::Dark,
            (_, None) => Appearance::Light,
            _ => appearance,
        };
        Self {
            theme: chosen,
            appearance,
        }
    }

    /// Whether the page starts in dark mode; with `System`, a script in the
    /// page head follows the system instead.
    pub fn dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }
}

const THEME_SETTING: &str = "appearance.theme";
const APPEARANCE_SETTING: &str = "appearance.mode";

/// The instance's default theme and appearance.
pub fn defaults(conn: &Connection) -> AppResult<(String, String)> {
    Ok((
        store::setting(conn, THEME_SETTING)?.unwrap_or_else(|| DEFAULT_THEME.to_owned()),
        store::setting(conn, APPEARANCE_SETTING)?.unwrap_or_else(|| "system".to_owned()),
    ))
}

pub fn validate(theme: &str, appearance: &str) -> AppResult<()> {
    if !theme.is_empty() && find(theme).is_none() {
        return Err(AppError::bad_request("Pick one of the themes."));
    }
    if !appearance.is_empty() && Appearance::parse(appearance).is_none() {
        return Err(AppError::bad_request("Pick system, light or dark."));
    }
    Ok(())
}

pub fn set_defaults(conn: &Connection, theme: &str, appearance: &str) -> AppResult<()> {
    validate(theme, appearance)?;
    store::set_setting(conn, THEME_SETTING, theme)?;
    store::set_setting(conn, APPEARANCE_SETTING, appearance)
}

fn declarations(palette: &Palette) -> String {
    palette
        .tokens()
        .iter()
        .fold(String::new(), |mut css, (name, value)| {
            // Writing to a String cannot fail.
            let _ = write!(css, "--{name}:{value};");
            css
        })
}

/// Every theme's tokens, with Sideporch's as the fallback.
pub static CSS: LazyLock<String> = LazyLock::new(|| {
    let mut css = String::new();
    for (index, theme) in THEMES.iter().enumerate() {
        let (light, dark) = match (theme.light, theme.dark) {
            (Some(light), Some(dark)) => (light, dark),
            (Some(only), None) | (None, Some(only)) => (only, only),
            (None, None) => continue,
        };
        let fallback = if index == 0 { ":root," } else { "" };
        let _ = writeln!(
            css,
            "{fallback}[data-theme=\"{id}\"]{{{light}}}\n{dark_root}[data-theme=\"{id}\"].dark{{{dark}}}",
            id = theme.id,
            light = declarations(&light),
            dark = declarations(&dark),
            dark_root = if index == 0 { ":root.dark," } else { "" },
        );
    }
    css
});

/// Changes with the themes, for cache-busting their stylesheet.
pub static VERSION: LazyLock<String> = LazyLock::new(|| {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    CSS.hash(&mut hasher);
    format!("{:x}", hasher.finish())
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_choices_within_what_themes_offer() {
        let choice = Choice::resolve("dracula", "light", DEFAULT_THEME, "system");
        assert_eq!(
            (choice.theme.id, choice.appearance),
            ("dracula", Appearance::Dark)
        );
        let choice = Choice::resolve("", "", "nord", "dark");
        assert_eq!(
            (choice.theme.id, choice.appearance),
            ("nord", Appearance::Dark)
        );
        let choice = Choice::resolve("nope", "", "also-nope", "");
        assert_eq!(
            (choice.theme.id, choice.appearance),
            ("sideporch", Appearance::System)
        );
        assert!(CSS.contains(r#"[data-theme="dracula"].dark{--floor:#21222C;"#));
        assert!(CSS.starts_with(":root,[data-theme=\"sideporch\"]"));
        assert_eq!(THEMES.len(), 20);
    }
}
