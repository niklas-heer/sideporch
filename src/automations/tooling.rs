//! Linting and formatting for automation scripts.
//!
//! Syntax errors come from `full_moon`, a Lua parser in Rust. Lints come from
//! [selene](https://github.com/Kampfkarren/selene) with a standard library
//! that matches the sandbox: Lua 5.4's safe libraries plus the `sideporch`
//! table from [`super::api`]. Formatting uses
//! [StyLua](https://github.com/JohnnyMorganz/StyLua).

use std::sync::LazyLock;

use selene_lib::{
    Checker, CheckerConfig,
    lints::Severity as SeleneSeverity,
    standard_library::{Field, StandardLibrary},
};
use serde::Serialize;

use super::api;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A problem in a script. Lines and columns start at 1; columns and the
/// `start`/`end` offsets count UTF-16 code units, like browser text fields.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub line: usize,
    pub column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub start: usize,
    pub end: usize,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        write!(
            f,
            "{}:{}: {severity}[{}]: {}",
            self.line, self.column, self.code, self.message
        )
    }
}

/// Maps byte offsets in a source to lines, columns and UTF-16 offsets.
struct Positions<'a> {
    source: &'a str,
}

impl Positions<'_> {
    /// (line, column, UTF-16 offset) for byte offset `at`.
    fn locate(&self, at: usize) -> (usize, usize, usize) {
        let mut line = 1_usize;
        let mut column = 1_usize;
        let mut utf16 = 0_usize;
        for (index, character) in self.source.char_indices() {
            if index >= at {
                break;
            }
            let width = character.len_utf16();
            utf16 = utf16.saturating_add(width);
            if character == '\n' {
                line = line.saturating_add(1);
                column = 1;
            } else {
                column = column.saturating_add(width);
            }
        }
        (line, column, utf16)
    }

    fn diagnostic(
        &self,
        severity: Severity,
        code: &str,
        message: String,
        (start, end): (usize, usize),
    ) -> Diagnostic {
        let (line, column, start) = self.locate(start);
        let (end_line, end_column, end) = self.locate(end.max(1));
        Diagnostic {
            severity,
            code: code.to_owned(),
            message,
            line,
            column,
            end_line,
            end_column,
            start,
            end: end.max(start),
        }
    }
}

/// Lua 5.4 as the sandbox provides it, plus the `sideporch` table.
fn standard_library() -> Option<StandardLibrary> {
    let mut library = StandardLibrary::from_name("lua53")?;
    library.globals.retain(|name, _| {
        !api::REMOVED
            .iter()
            .any(|removed| name == removed || name.starts_with(&format!("{removed}.")))
    });
    // Lua 5.4 additions that selene's Lua 5.3 library lacks.
    for name in ["warn", "coroutine.close"] {
        library
            .globals
            .insert(name.to_owned(), serde_json::from_value(any_args()).ok()?);
    }
    let sideporch: std::collections::BTreeMap<String, Field> =
        serde_json::from_value(api::selene_globals()).ok()?;
    library.globals.extend(sideporch);
    Some(library)
}

fn any_args() -> serde_json::Value {
    serde_json::json!({ "args": [{ "type": "..." }] })
}

static CHECKER: LazyLock<Option<Checker<serde_json::Value>>> = LazyLock::new(|| {
    let library = standard_library()?;
    let mut config = CheckerConfig::default();
    // Layout is the formatter's job.
    config.lints.insert(
        "multiple_statements".to_owned(),
        selene_lib::LintVariation::Allow,
    );
    match Checker::new(config, library) {
        Ok(checker) => Some(checker),
        Err(error) => {
            tracing::error!(%error, "could not set up the Lua linter");
            None
        }
    }
});

fn parse(source: &str) -> Result<full_moon::ast::Ast, Vec<Diagnostic>> {
    let positions = Positions { source };
    full_moon::parse_fallible(source, full_moon::LuaVersion::lua54())
        .into_result()
        .map_err(|errors| {
            errors
                .iter()
                .map(|error| {
                    let (start, end) = error.range();
                    positions.diagnostic(
                        Severity::Error,
                        "syntax",
                        error.error_message().into_owned(),
                        (start.bytes(), end.bytes()),
                    )
                })
                .collect()
        })
}

/// Syntax errors, or else lint findings, in source order.
pub fn lint(source: &str) -> Vec<Diagnostic> {
    let ast = match parse(source) {
        Ok(ast) => ast,
        Err(errors) => return errors,
    };
    let Some(checker) = CHECKER.as_ref() else {
        return Vec::new();
    };
    let positions = Positions { source };
    let mut diagnostics: Vec<Diagnostic> = checker
        .test_on(&ast)
        .into_iter()
        .filter_map(|found| {
            let severity = match found.severity {
                SeleneSeverity::Error => Severity::Error,
                SeleneSeverity::Warning => Severity::Warning,
                SeleneSeverity::Allow => return None,
            };
            let diagnostic = found.diagnostic;
            let (start, end) = diagnostic.primary_label.range;
            let message =
                sandbox_message(diagnostic.code, &diagnostic.message).unwrap_or(diagnostic.message);
            Some(positions.diagnostic(
                severity,
                diagnostic.code,
                message,
                (usize_from(start), usize_from(end)),
            ))
        })
        .collect();
    diagnostics.sort_by_key(|diagnostic| (diagnostic.start, diagnostic.end));
    diagnostics
}

fn usize_from(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

/// Explains why a well-known global is missing.
fn sandbox_message(code: &str, message: &str) -> Option<String> {
    if code != "undefined_variable" {
        return None;
    }
    let name = message.strip_prefix('`')?.split('`').next()?;
    api::REMOVED.contains(&name).then(|| {
        format!(
            "`{name}` is not available: automations run in a sandbox without file, process or network access"
        )
    })
}

/// The script in `StyLua`'s style with two-space indentation, or its syntax
/// errors.
pub fn format(source: &str) -> Result<String, Vec<Diagnostic>> {
    parse(source)?;
    let config = stylua_lib::Config {
        syntax: stylua_lib::LuaVersion::Lua54,
        indent_type: stylua_lib::IndentType::Spaces,
        indent_width: 2,
        column_width: 100,
        ..stylua_lib::Config::default()
    };
    stylua_lib::format_code(source, config, None, stylua_lib::OutputVerification::Full).map_err(
        |error| {
            vec![Diagnostic {
                severity: Severity::Error,
                code: "format".to_owned(),
                message: error.to_string(),
                line: 1,
                column: 1,
                end_line: 1,
                end_column: 1,
                start: 0,
                end: 0,
            }]
        },
    )
}

/// True if any diagnostic is an error.
pub fn has_errors(diagnostics: &[Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(source: &str) -> Vec<String> {
        lint(source)
            .into_iter()
            .map(|diagnostic| diagnostic.code)
            .collect()
    }

    #[test]
    fn the_example_is_clean() {
        assert!(
            lint(super::super::EXAMPLE).is_empty(),
            "{:?}",
            lint(super::super::EXAMPLE)
        );
    }

    #[test]
    fn reports_syntax_errors_with_positions() {
        let found = lint("local x = 1\nif x then\n  print(x)\n");
        assert_eq!(found.len(), 1, "{found:?}");
        let error = found.first().unwrap();
        assert_eq!(error.code, "syntax");
        assert_eq!(error.severity, Severity::Error);
        assert!(error.line >= 2, "{error:?}");
    }

    #[test]
    fn knows_the_sandbox() {
        assert_eq!(codes("os.execute('rm -rf /')"), ["undefined_variable"]);
        let message = lint("print(io.open('x'))").remove(0).message;
        assert!(message.contains("sandbox"), "{message}");
        assert!(codes("print(string.format('%d', 1), table.concat({}), utf8.char(65))").is_empty());
        assert!(codes("warn('careful')").is_empty());
    }

    #[test]
    fn knows_the_sideporch_api() {
        assert!(
            codes("sideporch.on_reaction(function(e) sideporch.react(e.message, e.emoji) end)")
                .is_empty()
        );
        assert_eq!(
            codes("sideporch.pots('general', 'hi')"),
            ["incorrect_standard_library_use"]
        );
        assert_eq!(
            codes("sideporch.post('general')"),
            ["incorrect_standard_library_use"]
        );
        assert_eq!(codes("sideporch.now()"), ["must_use"]);
    }

    #[test]
    fn warns_about_unused_locals() {
        assert_eq!(codes("local unused = 1"), ["unused_variable"]);
    }

    #[test]
    fn counts_columns_in_utf16() {
        let found = lint("print(\"😀\") nope()");
        let error = found.first().unwrap();
        assert_eq!(error.code, "undefined_variable");
        // The emoji is one character but two UTF-16 units.
        assert_eq!(error.column, 13);
        assert_eq!(error.start, 12);
    }

    #[test]
    fn formats_with_two_spaces() {
        let formatted = format("sideporch.on_message(function(msg) if msg.text=='!ping' then sideporch.reply(msg,'pong') end end)").unwrap();
        assert_eq!(
            formatted,
            "sideporch.on_message(function(msg)\n  if msg.text == \"!ping\" then\n    sideporch.reply(msg, \"pong\")\n  end\nend)\n"
        );
        assert!(format("if then").is_err());
    }
}
