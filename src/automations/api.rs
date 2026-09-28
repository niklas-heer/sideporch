//! The automation API, described once. The linter's standard library, the
//! editor's completions, the in-page reference, the AI prompt and the MCP
//! reference are all generated from this table.

use std::fmt::Write as _;

use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Any,
    Function,
    Number,
    String,
    Table,
}

impl Kind {
    /// The type name selene's standard library format uses.
    const fn selene(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Function => "function",
            Self::Number => "number",
            Self::String => "string",
            Self::Table => "table",
        }
    }
}

#[derive(Debug)]
pub struct Arg {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
}

const fn arg(name: &'static str, kind: Kind) -> Arg {
    Arg {
        name,
        kind,
        required: true,
    }
}

const fn optional(name: &'static str, kind: Kind) -> Arg {
    Arg {
        name,
        kind,
        required: false,
    }
}

#[derive(Debug)]
pub struct Function {
    /// The full dotted name, such as `sideporch.post`.
    pub name: &'static str,
    pub args: &'static [Arg],
    /// What the editor inserts after `sideporch.`; `$0` marks the cursor.
    pub snippet: &'static str,
    pub doc: &'static str,
    /// Calling it only for its side effects is a mistake.
    pub must_use: bool,
}

impl Function {
    /// `sideporch.post(channel, text, options?)`
    pub fn signature(&self) -> String {
        let args: Vec<String> = self
            .args
            .iter()
            .map(|arg| {
                if arg.required {
                    arg.name.to_owned()
                } else {
                    format!("{}?", arg.name)
                }
            })
            .collect();
        format!("{}({})", self.name, args.join(", "))
    }
}

pub const FUNCTIONS: &[Function] = &[
    Function {
        name: "sideporch.on_message",
        args: &[arg("handler", Kind::Function)],
        snippet: "on_message(function(msg)\n  $0\nend)",
        doc: "Calls `handler(msg)` for every new message in a public channel. Messages from automations never trigger it.",
        must_use: false,
    },
    Function {
        name: "sideporch.on_reaction",
        args: &[arg("handler", Kind::Function)],
        snippet: "on_reaction(function(event)\n  $0\nend)",
        doc: "Calls `handler(event)` when someone adds or removes a reaction on a message in a public channel.",
        must_use: false,
    },
    Function {
        name: "sideporch.on_webhook",
        args: &[arg("handler", Kind::Function)],
        snippet: "on_webhook(function(request)\n  $0\n  return { status = 200, body = \"ok\" }\nend)",
        doc: "Calls `handler(request)` for each HTTP request to this automation's webhook URL, shown in the editor. What the handler returns becomes the response. A script can register one webhook handler.",
        must_use: false,
    },
    Function {
        name: "sideporch.every",
        args: &[arg("seconds", Kind::Number), arg("handler", Kind::Function)],
        snippet: "every(3600, function()\n  $0\nend)",
        doc: "Calls `handler()` every `seconds` seconds, at least 10. The first call comes one interval after the script loads.",
        must_use: false,
    },
    Function {
        name: "sideporch.post",
        args: &[
            arg("channel", Kind::String),
            arg("text", Kind::String),
            optional("options", Kind::Table),
        ],
        snippet: "post(\"$0\", \"\")",
        doc: "Posts `text` to the public channel named `channel` (with or without `#`), under the automation's name. `options.thread` is a message id to answer in that thread. Text uses Slack-style formatting such as `*bold*` and `<https://example.com|links>`.",
        must_use: false,
    },
    Function {
        name: "sideporch.reply",
        args: &[arg("message", Kind::Table), arg("text", Kind::String)],
        snippet: "reply(msg, \"$0\")",
        doc: "Answers `message` in its thread. `message` is a message table from `on_message` or a reaction event's `message`.",
        must_use: false,
    },
    Function {
        name: "sideporch.react",
        args: &[arg("message", Kind::Table), arg("emoji", Kind::String)],
        snippet: "react(msg, \"$0\")",
        doc: "Adds the reaction `emoji` (a name such as `\"white_check_mark\"` or a custom emoji) to `message`. Reacting twice with the same emoji has no further effect.",
        must_use: false,
    },
    Function {
        name: "sideporch.get",
        args: &[arg("key", Kind::String)],
        snippet: "get(\"$0\")",
        doc: "Returns the string saved under `key` for this automation, or `nil`. Saved data survives restarts and edits.",
        must_use: true,
    },
    Function {
        name: "sideporch.set",
        args: &[arg("key", Kind::String), optional("value", Kind::Any)],
        snippet: "set(\"$0\", value)",
        doc: "Saves `value` under `key` as a string (numbers and booleans are converted; use `sideporch.json.encode` for tables). `nil` deletes the key. Keys take up to 200 bytes and values up to 64 kB.",
        must_use: false,
    },
    Function {
        name: "sideporch.now",
        args: &[],
        snippet: "now()$0",
        doc: "Returns the current Unix time in seconds.",
        must_use: true,
    },
    Function {
        name: "sideporch.json.encode",
        args: &[arg("value", Kind::Any)],
        snippet: "json.encode($0)",
        doc: "Returns `value` as a JSON string. Empty tables become `[]` only when made with `sideporch.json.array()`; otherwise `{}`.",
        must_use: true,
    },
    Function {
        name: "sideporch.json.decode",
        args: &[arg("text", Kind::String)],
        snippet: "json.decode($0)",
        doc: "Parses a JSON string into Lua values. JSON `null` becomes `sideporch.json.null`. Fails on invalid JSON; use `pcall` to handle that.",
        must_use: true,
    },
    Function {
        name: "sideporch.json.array",
        args: &[optional("items", Kind::Table)],
        snippet: "json.array($0)",
        doc: "Marks a table as a JSON array, so an empty one encodes as `[]`.",
        must_use: true,
    },
];

/// Fields of the tables handlers receive, and of the webhook response.
pub const EVENTS: &[(&str, &[(&str, &str)])] = &[
    (
        "msg (on_message, reply, react)",
        &[
            ("id", "message id"),
            ("channel", "channel name, without `#`"),
            ("channel_id", "channel id"),
            ("text", "the message text as written"),
            ("author", "display name of the author"),
            (
                "username",
                "the author's username; `nil` for bots and webhooks",
            ),
            ("is_bot", "`true` for webhook posts"),
            (
                "thread_id",
                "id of the thread's first message, or `nil` outside threads",
            ),
        ],
    ),
    (
        "event (on_reaction)",
        &[
            ("emoji", "emoji name, such as `\"thumbsup\"`"),
            ("added", "`true` when added, `false` when removed"),
            ("user", "display name of the person reacting"),
            ("username", "their username"),
            ("message", "the message, as a `msg` table"),
        ],
    ),
    (
        "request (on_webhook)",
        &[
            ("method", "HTTP method, such as `\"POST\"`"),
            (
                "path",
                "the part of the URL after the webhook token, such as `\"/deploy\"`, or `\"\"`",
            ),
            ("query", "table of query parameters"),
            ("headers", "table of headers, names in lower case"),
            ("body", "the raw body as a string"),
            ("json", "the body parsed as JSON, or `nil`"),
        ],
    ),
    (
        "webhook response (return value of the on_webhook handler)",
        &[
            ("nil", "answers 204 No Content"),
            ("a string", "answers 200 with that text"),
            (
                "{ status = 200, body = \"text\" }",
                "status code (default 200) and a text body",
            ),
            ("{ json = value }", "a JSON body"),
        ],
    ),
];

/// Globals the sandbox removes. Lua scripts that use them fail to lint.
pub const REMOVED: &[&str] = &[
    "collectgarbage",
    "debug",
    "dofile",
    "getfenv",
    "io",
    "load",
    "loadfile",
    "loadstring",
    "module",
    "os",
    "package",
    "require",
    "setfenv",
    "string.dump",
    "unpack",
];

pub const LIMITS: &str = "Each handler call may run 2,000,000 Lua instructions and post 20 messages; each script may use 16 MB of memory. \
Scripts have Lua 5.4's `string`, `table`, `math`, `utf8` and `coroutine` libraries and the base functions, but no file, process or network access. \
`print(...)` writes to the automation's run log.";

/// The reference as Markdown, for people, the AI prompt and MCP.
pub fn reference() -> String {
    let mut text = String::from("# Sideporch automation API\n\n");
    text.push_str(LIMITS);
    text.push_str("\n\n## Functions\n\n");
    for function in FUNCTIONS {
        // Writing to a String cannot fail.
        let _ = writeln!(text, "- `{}`: {}", function.signature(), function.doc);
    }
    text.push_str("\n## Tables\n");
    for (name, fields) in EVENTS {
        let _ = write!(text, "\n### {name}\n\n");
        for (field, doc) in *fields {
            let _ = writeln!(text, "- `{field}`: {doc}");
        }
    }
    text
}

/// Completions for the editor, as JSON.
pub fn completions() -> Value {
    Value::Array(
        FUNCTIONS
            .iter()
            .map(|function| {
                json!({
                    "name": function.name.trim_start_matches("sideporch."),
                    "signature": function.signature(),
                    "snippet": function.snippet,
                    "doc": function.doc,
                })
            })
            .collect(),
    )
}

/// The `sideporch` table in selene's standard library format.
pub fn selene_globals() -> Value {
    let mut globals = serde_json::Map::new();
    for function in FUNCTIONS {
        let args: Vec<Value> = function
            .args
            .iter()
            .map(|arg| {
                if arg.required {
                    json!({ "type": arg.kind.selene() })
                } else {
                    json!({ "type": arg.kind.selene(), "required": false })
                }
            })
            .collect();
        globals.insert(
            function.name.to_owned(),
            json!({ "args": args, "must_use": function.must_use }),
        );
    }
    globals.insert(
        "sideporch.json.null".to_owned(),
        json!({ "property": "read-only" }),
    );
    Value::Object(globals)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_lists_every_function() {
        let reference = reference();
        for function in FUNCTIONS {
            assert!(
                reference.contains(&function.signature()),
                "{}",
                function.name
            );
        }
        assert!(reference.contains("sideporch.post(channel, text, options?)"));
    }
}
