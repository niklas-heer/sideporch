// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Automations as a platform: events with filters, slash commands,
//! libraries, outgoing HTTP, secrets, settings, and the MCP tools around
//! them, driven through a real server.

mod common;

use std::sync::{Arc, Mutex};

use axum::{Json, Router, http::HeaderMap, routing::get};
use common::{Browser, admin, between, home_channel, invite, location, start};
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn save(admin: &Browser, name: &str, source: &str) -> String {
    let response = admin
        .post(
            "/automations",
            &[("name", name), ("source", source), ("enabled", "on")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER, "saving {name}");
    location(&response)
}

async fn test_run(admin: &Browser, source: &str, trigger: Value, http: bool) -> Value {
    let response = admin
        .post_json(
            "/automations/test",
            &json!({ "source": source, "trigger": trigger, "http": http }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

#[tokio::test]
async fn slash_commands_answer_privately() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let path = save(
        &admin,
        "Deploy bot",
        r#"sideporch.command("deploy", { description = "Deploy a service", usage = "<service>" }, function(cmd)
  if #cmd.args == 0 then
    sideporch.respond(cmd, "Which service? Try `/deploy garden`.")
    return
  end
  sideporch.respond(cmd, "Deploying **" .. cmd.args[1] .. "** for " .. cmd.user)
  sideporch.post(cmd.channel, cmd.username .. " started a deploy of " .. cmd.args[1])
end)"#,
    )
    .await;

    let answer = admin.type_message(general, "/deploy garden").await;
    assert_eq!(answer.status(), StatusCode::OK);
    let notices: Value = answer.json().await.unwrap();
    let notice = notices["ephemeral"][0].as_str().unwrap();
    assert!(notice.contains("Only visible to you"), "{notice}");
    assert!(
        notice.contains("Deploying <strong>garden</strong> for Ada Admin"),
        "{notice}"
    );
    // The command itself is not a message, but its public post is.
    let page = admin
        .wait_for(&format!("/c/{general}"), "ada started a deploy of garden")
        .await;
    assert!(!page.contains("/deploy garden"));

    let help: Value = admin
        .type_message(general, "/help")
        .await
        .json()
        .await
        .unwrap();
    let help = help["ephemeral"][0].as_str().unwrap();
    assert!(
        help.contains("/deploy &lt;service&gt;") && help.contains("Deploy bot"),
        "{help}"
    );
    let commands: Value = admin.get("/commands").await.json().await.unwrap();
    assert_eq!(commands[0]["name"], "deploy");

    // Text that is not a registered command is an ordinary message.
    assert_eq!(
        admin
            .type_message(general, "/etc/hosts is broken")
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        admin.type_message(general, "/nothing here").await.status(),
        StatusCode::NO_CONTENT
    );
    assert!(admin.page(&path).await.contains("/deploy garden by @ada"));

    // A second automation cannot take the same command.
    let copy = save(
        &admin,
        "Copycat",
        r#"sideporch.command("deploy", function(cmd) end)"#,
    )
    .await;
    assert!(
        admin
            .page(&copy)
            .await
            .contains("/deploy is already registered by Deploy bot")
    );
}

#[tokio::test]
async fn filtered_events_greet_members_and_channels() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    save(
        &admin,
        "Porch host",
        r#"sideporch.on("member_joined", function(event)
  sideporch.post("general", "Welcome, " .. event.user .. "!")
end)
sideporch.on("channel_created", function(event)
  sideporch.post("general", "New channel: #" .. event.channel .. " by @" .. event.username)
end)
sideporch.on("message", { pattern = "^!echo (.+)", thread = false }, function(msg)
  sideporch.reply(msg, msg.text:match("^!echo (.+)"))
end)"#,
    )
    .await;

    invite(&server, &admin, "Mo Member", "mo").await;
    admin
        .wait_for(&format!("/c/{general}"), "Welcome, Mo Member!")
        .await;
    let created = admin.post("/channels", &[("name", "garden-club")]).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    admin
        .wait_for(&format!("/c/{general}"), "New channel: #garden-club by")
        .await;

    admin.send(general, "!echo porch lights", None).await;
    let echo = common::last_message_id(&admin.page(&format!("/c/{general}")).await);
    admin
        .wait_for(&format!("/c/{general}/t/{echo}"), ">porch lights<")
        .await;
}

#[tokio::test]
async fn libraries_share_code() {
    let server = start().await;
    let admin = admin(&server).await;
    let bad = admin
        .post(
            "/automations",
            &[
                ("name", "My Lib"),
                ("source", "return {}"),
                ("kind", "library"),
            ],
        )
        .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    let library = admin
        .post(
            "/automations",
            &[
                ("name", "greetings"),
                ("source", "local M = {}\nfunction M.hello(name) return 'Hello, ' .. name .. '!' end\nreturn M"),
                ("kind", "library"),
                ("enabled", "on"),
            ],
        )
        .await;
    assert_eq!(library.status(), StatusCode::SEE_OTHER);
    let list = admin.page("/automations").await;
    assert!(
        list.contains("require(&quot;greetings&quot;)") || list.contains("require(\"greetings\")")
    );

    let report = test_run(
        &admin,
        "local greetings = require('greetings')\nsideporch.on('message', function(msg) sideporch.reply(msg, greetings.hello(msg.author)) end)",
        json!({ "kind": "message", "text": "hi" }),
        false,
    )
    .await;
    assert_eq!(report["ok"], true, "{report}");
    assert_eq!(
        report["log"][0],
        "→ post in #general (thread 1): Hello, Test Person!"
    );
    let exports = test_run(
        &admin,
        "local M = {}\nfunction M.a() end\nreturn M",
        json!({ "kind": "load" }),
        false,
    )
    .await;
    assert_eq!(exports["exports"], json!(["a"]));
}

/// A tiny API that records the requests it gets.
async fn fake_api() -> (String, Arc<Mutex<Vec<HeaderMap>>>) {
    let seen: Arc<Mutex<Vec<HeaderMap>>> = Arc::default();
    let record = Arc::clone(&seen);
    let app = Router::new().route(
        "/status",
        get(move |headers: HeaderMap| {
            let record = Arc::clone(&record);
            async move {
                record.lock().unwrap().push(headers);
                Json(json!({ "healthy": true, "version": "1.2.3" }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, seen)
}

#[tokio::test]
async fn scripts_call_apis_with_secrets() {
    let server = start().await;
    let admin = admin(&server).await;
    let (api, seen) = fake_api().await;
    let source = format!(
        r#"sideporch.on("message", function(msg)
  local response = sideporch.http.get("{api}/status?key=" .. sideporch.secret("API_TOKEN"), {{
    headers = {{ Authorization = "Bearer " .. sideporch.secret("API_TOKEN") }},
  }})
  print(response.status, response.json.version, sideporch.secret("API_TOKEN"))
end)"#
    );
    let trigger = json!({ "kind": "message", "text": "status?" });
    let stored = admin
        .post(
            "/settings/secrets",
            &[("name", "API_TOKEN"), ("value", "tok-12345")],
        )
        .await;
    assert_eq!(stored.status(), StatusCode::OK);
    let secrets = admin.page("/settings/secrets").await;
    assert!(secrets.contains("API_TOKEN") && !secrets.contains("tok-12345"));

    // Internal addresses are refused until an admin allows them.
    let blocked = test_run(&admin, &source, trigger.clone(), true).await;
    assert!(
        blocked["error"]
            .as_str()
            .unwrap()
            .contains("internal address"),
        "{blocked}"
    );
    let off = test_run(&admin, &source, trigger.clone(), false).await;
    assert!(off["error"].as_str().unwrap().contains("switched off"));

    let settings = admin
        .post(
            "/settings/automations",
            &[
                ("timezone", "Europe/Berlin"),
                ("allow_private_network", "on"),
            ],
        )
        .await;
    assert_eq!(settings.status(), StatusCode::OK);
    let wrong_zone = admin
        .post("/settings/automations", &[("timezone", "Mars/Base")])
        .await;
    assert_eq!(wrong_zone.status(), StatusCode::BAD_REQUEST);

    let report = test_run(&admin, &source, trigger, true).await;
    assert_eq!(report["ok"], true, "{report}");
    let log: Vec<&str> = report["log"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect();
    assert!(
        log[0].starts_with(&format!("→ GET {api}/status 200")),
        "{log:?}"
    );
    // The secret reaches the API but never the log.
    assert_eq!(log[1], "200\t1.2.3\t[secret API_TOKEN]");
    assert!(!report.to_string().contains("tok-12345"));
    let headers = seen.lock().unwrap()[0].clone();
    assert_eq!(headers["authorization"], "Bearer tok-12345");
    assert!(
        headers["user-agent"]
            .to_str()
            .unwrap()
            .starts_with("Sideporch/")
    );
}

async fn rpc(base: &str, token: &str, method: &str, params: Value) -> Value {
    let response = reqwest::Client::new()
        .post(format!("{base}/mcp"))
        .bearer_auth(token)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

async fn call(base: &str, token: &str, tool: &str, arguments: Value) -> Value {
    let answer = rpc(
        base,
        token,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
    .await;
    let result = answer["result"].clone();
    assert_ne!(result["isError"], true, "{tool}: {result}");
    result["structuredContent"].clone()
}

/// A server with an admin and an MCP token for them.
async fn agent() -> (common::Server, Browser, i64, String) {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let page = admin
        .post("/settings/mcp/tokens", &[("name", "Agent")])
        .await
        .text()
        .await
        .unwrap();
    let token = format!("sp_{}", between(&page, "data-copy=\"sp_", "\""));
    (server, admin, general, token)
}

#[tokio::test]
async fn agents_schedule_run_and_inspect_automations_over_mcp() {
    let (server, admin, general, token) = agent().await;
    let base = &server.base;
    let cron = call(
        base,
        &token,
        "explain_cron",
        json!({ "expression": "0 9 * * mon-fri", "timezone": "UTC", "count": 3 }),
    )
    .await;
    assert_eq!(cron["next_runs"].as_array().unwrap().len(), 3);
    assert!(cron["next_runs"][0].as_str().unwrap().contains("09:00"));

    call(
        base,
        &token,
        "set_secret",
        json!({ "name": "WEATHER_KEY", "value": "k-98765" }),
    )
    .await;
    let secrets = call(base, &token, "list_secrets", json!({})).await;
    assert_eq!(secrets["secrets"][0]["name"], "WEATHER_KEY");
    assert!(!secrets.to_string().contains("k-98765"));

    let source = "sideporch.cron('@hourly', function()\n  sideporch.post('general', 'Hourly check-in')\nend)\n";
    let saved = call(
        base,
        &token,
        "save_automation",
        json!({ "name": "Hourly", "source": source, "enabled": true }),
    )
    .await;
    let id = saved["id"].as_i64().unwrap();
    assert_eq!(
        saved["listens_to"]["schedules"][0]["description"],
        "cron `@hourly` (UTC)"
    );
    call(
        base,
        &token,
        "run_automation",
        json!({ "id": id, "trigger": { "kind": "timer" } }),
    )
    .await;
    admin
        .wait_for(&format!("/c/{general}"), "Hourly check-in")
        .await;
    let messages = call(
        base,
        &token,
        "read_messages",
        json!({ "channel": "general" }),
    )
    .await;
    assert_eq!(
        messages["messages"].as_array().unwrap().last().unwrap()["text"],
        "Hourly check-in"
    );

    call(
        base,
        &token,
        "set_data",
        json!({ "id": id, "key": "count", "value": "3" }),
    )
    .await;
    let data = call(base, &token, "list_data", json!({ "id": id })).await;
    assert_eq!(data["data"]["count"], "3");
}

#[tokio::test]
async fn agents_restore_versions_and_read_resources_over_mcp() {
    let (server, _admin, _general, token) = agent().await;
    let base = &server.base;
    let source = "sideporch.cron('@hourly', function()\n  sideporch.post('general', 'Hourly check-in')\nend)\n";
    let saved = call(
        base,
        &token,
        "save_automation",
        json!({ "name": "Hourly", "source": source }),
    )
    .await;
    let id = saved["id"].as_i64().unwrap();
    let second = format!("{source}-- tweaked\n");
    call(
        base,
        &token,
        "save_automation",
        json!({ "id": id, "name": "Hourly", "source": second }),
    )
    .await;
    let versions = call(base, &token, "list_versions", json!({ "id": id })).await;
    let oldest = versions["versions"].as_array().unwrap().last().unwrap()["version_id"]
        .as_i64()
        .unwrap();
    call(
        base,
        &token,
        "restore_version",
        json!({ "id": id, "version_id": oldest }),
    )
    .await;
    let restored = rpc(
        base,
        &token,
        "resources/read",
        json!({ "uri": format!("sideporch://automations/{id}") }),
    )
    .await;
    assert_eq!(restored["result"]["contents"][0]["text"], source);

    let off = call(
        base,
        &token,
        "set_enabled",
        json!({ "id": id, "enabled": false }),
    )
    .await;
    assert_eq!(off["enabled"], false);
    let resources = rpc(base, &token, "resources/list", json!({})).await;
    assert_eq!(
        resources["result"]["resources"][0]["uri"],
        "sideporch://reference"
    );
    let prompt = rpc(
        base,
        &token,
        "prompts/get",
        json!({ "name": "write_automation", "arguments": { "task": "greet people" } }),
    )
    .await;
    assert!(
        prompt["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("greet people")
    );
    let settings = call(
        base,
        &token,
        "update_settings",
        json!({ "timezone": "Europe/Berlin" }),
    )
    .await;
    assert_eq!(settings["timezone"], "Europe/Berlin");
}
