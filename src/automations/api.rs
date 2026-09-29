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
        name: "sideporch.on",
        args: &[
            arg("event", Kind::String),
            arg("filter_or_handler", Kind::Any),
            optional("handler", Kind::Function),
        ],
        snippet: "on(\"message\", { channel = \"$0\" }, function(msg)\n  \nend)",
        doc: "Calls `handler(event)` when `event` happens: `\"message\"` (new messages in public channels), `\"message_changed\"` (edited; the table has the new text), `\"message_deleted\"` (the table has what it said), `\"reaction_added\"`, `\"reaction_removed\"`, `\"reaction\"` (both), `\"member_joined\"`, `\"channel_created\"` or `\"button\"` (someone clicked a button under one of this automation's messages; only the automation that posted it hears about it, and `pattern` matches the button's value). An optional filter table between them narrows it down: `channel` (name), `pattern` (a Lua pattern the message text must match), `emoji`, `user` (username) and `thread` (`true` for replies in threads, `false` for the rest). Posts and reactions from automations never trigger handlers. The event table's `event` field names the event.",
        must_use: false,
    },
    Function {
        name: "sideporch.cron",
        args: &[
            arg("expression", Kind::String),
            arg("options_or_handler", Kind::Any),
            optional("handler", Kind::Function),
        ],
        snippet: "cron(\"0 9 * * mon-fri\", function()\n  $0\nend)",
        doc: "Calls `handler()` on a cron schedule: `minute hour day-of-month month day-of-week`, with ranges, lists, steps (`*/15`), names (`mon-fri`, `jan`) and shortcuts such as `@hourly` and `@daily`. Times are in the instance's automation time zone, or pass `{ timezone = \"Europe/Berlin\" }` before the handler.",
        must_use: false,
    },
    Function {
        name: "sideporch.command",
        args: &[
            arg("name", Kind::String),
            arg("options_or_handler", Kind::Any),
            optional("handler", Kind::Function),
        ],
        snippet: "command(\"$0\", { description = \"\", usage = \"\" }, function(cmd)\n  sideporch.respond(cmd, \"\")\nend)",
        doc: "Registers the slash command `/name`, which people type in any channel. `handler(cmd)` gets `cmd.text` (everything after the name), `cmd.args` (its words), `cmd.user`, `cmd.username`, `cmd.channel` and `cmd.channel_id`. Answer privately with `sideporch.respond`, or publicly with `sideporch.post` or `sideporch.reply(cmd, …)`. Options: `description` and `usage` for `/help` and the composer's suggestions. Each command name belongs to one automation.",
        must_use: false,
    },
    Function {
        name: "sideporch.respond",
        args: &[arg("cmd", Kind::Table), arg("text", Kind::String)],
        snippet: "respond(cmd, \"$0\")",
        doc: "Answers a slash command with Markdown that only the person who typed it sees.",
        must_use: false,
    },
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
        doc: "Posts `text` to the public channel named `channel` (with or without `#`), under the automation's name. `options.thread` is a message id to answer in that thread; `options.buttons` adds up to five buttons, each `{ label = \"Approve\", value = \"approve\", style = \"primary\" }` (style is `primary`, `danger` or left out), which people click to trigger `sideporch.on(\"button\", …)`. Text is GitHub-flavored Markdown, such as `**bold**`, `[a link](https://example.com)`, tables, and ```` ```mermaid ```` diagrams.",
        must_use: false,
    },
    Function {
        name: "sideporch.reply",
        args: &[
            arg("message", Kind::Table),
            arg("text", Kind::String),
            optional("options", Kind::Table),
        ],
        snippet: "reply(msg, \"$0\")",
        doc: "Answers `message` in its thread. `message` is a message table from `on_message` or a reaction event's `message`. `options.buttons` works as for `sideporch.post`.",
        must_use: false,
    },
    Function {
        name: "sideporch.update",
        args: &[
            arg("message", Kind::Table),
            optional("text", Kind::String),
            optional("options", Kind::Table),
        ],
        snippet: "update(click.message, \"$0\", { buttons = {} })",
        doc: "Changes a message this automation posted, such as `click.message` in a button handler: new text (or `nil` to keep it), and `options.buttons` to replace its buttons (`{}` removes them).",
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
        name: "sideporch.http.get",
        args: &[arg("url", Kind::String), optional("options", Kind::Table)],
        snippet: "http.get(\"$0\")",
        doc: "Sends a GET request and returns the response: `status`, `ok` (true for 2xx), `headers` (lower-case names), `body`, and `json` when the body is JSON. Options: `headers` and `timeout` (seconds, default 10, at most 30). Network errors raise an error; use `pcall` to handle them. Redirects are not followed. Private and loopback addresses are refused unless an admin allows them.",
        must_use: true,
    },
    Function {
        name: "sideporch.http.post",
        args: &[
            arg("url", Kind::String),
            optional("body", Kind::Any),
            optional("options", Kind::Table),
        ],
        snippet: "http.post(\"$0\", {})",
        doc: "Sends a POST request. A table body is sent as JSON; a string as is. Returns the same response table as `sideporch.http.get`.",
        must_use: false,
    },
    Function {
        name: "sideporch.http.request",
        args: &[arg("options", Kind::Table)],
        snippet: "http.request({ method = \"PUT\", url = \"$0\", headers = {}, json = {} })",
        doc: "Sends any request: `method`, `url`, `headers`, and `body` (a string) or `json` (a table), plus `timeout`. A call may make 10 requests.",
        must_use: false,
    },
    Function {
        name: "sideporch.secret",
        args: &[arg("name", Kind::String)],
        snippet: "secret(\"$0\")",
        doc: "Returns the secret `name`, such as an API token, or `nil`. Admins store secrets under Automations → Secrets, or set `SIDEPORCH_SECRET_<NAME>` in the environment. Secret values are replaced in run logs.",
        must_use: true,
    },
    Function {
        name: "require",
        args: &[arg("name", Kind::String)],
        snippet: "require(\"$0\")",
        doc: "Loads the library automation `name` once and returns what it returns, usually a table of functions. Libraries use the same `sideporch` API, acting for the automation that loaded them.",
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
        "event (member_joined)",
        &[
            ("user", "display name of the new member"),
            ("username", "their username"),
        ],
    ),
    (
        "event (channel_created)",
        &[
            ("channel", "the new channel's name"),
            ("channel_id", "its id"),
            ("user", "display name of who created it"),
            ("username", "their username"),
        ],
    ),
    (
        "cmd (command)",
        &[
            ("name", "the command, without `/`"),
            ("text", "everything typed after the name"),
            ("args", "the words of `text`, as a list"),
            ("user", "display name of who typed it"),
            ("username", "their username"),
            (
                "channel",
                "the channel's name, or `\"\"` in direct messages",
            ),
            ("channel_id", "the channel's id"),
            ("thread_id", "the thread it was typed in, or `nil`"),
        ],
    ),
    (
        "click (button)",
        &[
            ("value", "the button's value"),
            ("label", "the button's label"),
            ("user", "display name of who clicked it"),
            ("username", "their username"),
            ("message", "the message with the button, as a `msg` table"),
        ],
    ),
    (
        "response (sideporch.http)",
        &[
            ("status", "HTTP status code"),
            ("ok", "`true` for 2xx statuses"),
            ("headers", "table of headers, names in lower case"),
            ("body", "the body as a string"),
            ("json", "the body parsed, when it is JSON"),
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
    "setfenv",
    "string.dump",
    "unpack",
];

pub const LIMITS: &str = "Each handler call may run 2,000,000 Lua instructions, post 20 messages or reactions and make 10 HTTP requests; each script may use 16 MB of memory. \
Scripts have Lua 5.4's `string`, `table`, `math`, `utf8` and `coroutine` libraries, the base functions and `require` for libraries, but no file or process access; the network is reachable only through `sideporch.http`. \
Messages are GitHub-flavored Markdown. `print(...)` writes to the automation's run log.";

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
            .filter(|function| function.name.starts_with("sideporch."))
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

    /// The documentation's API page: `reference()` with the site's front
    /// matter instead of its title, kept out of the site's templating.
    fn website_page() -> String {
        let reference = reference();
        let body = reference
            .strip_prefix("# Sideporch automation API\n\n")
            .expect("the reference starts with its title");
        format!(
            "+++\n\
             title = \"Automation API\"\n\
             description = \"Every sideporch function automations can call, and the tables their handlers receive.\"\n\
             weight = 4\n\n\
             [extra]\n\
             edit_path = \"src/automations/api.rs\"\n\
             +++\n\n\
             <!-- Generated from src/automations/api.rs. Change the API there, then run\n     \
             SIDEPORCH_BLESS=1 cargo test website_page_is_current -->\n\n\
             {{% raw %}}\n{body}{{% endraw %}}\n"
        )
    }

    #[test]
    fn website_page_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("website/content/docs/integrations/automation-api.md");
        let expected = website_page();
        if std::env::var_os("SIDEPORCH_BLESS").is_some() {
            std::fs::write(&path, &expected).expect("write the API page");
            return;
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            actual == expected,
            "{} is out of date; run SIDEPORCH_BLESS=1 cargo test website_page_is_current",
            path.display()
        );
    }
}
