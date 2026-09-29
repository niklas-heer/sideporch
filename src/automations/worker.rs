//! One thread per running automation.
//!
//! A worker owns its script's Lua state and its own database connection.
//! It handles events, webhook requests, commands and timers one at a time,
//! so a slow script (waiting for an HTTP response, say) delays only itself.
//! Dropping the worker's job sender ends the thread after its current job.

use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use jiff::{Timestamp, tz::TimeZone};
use rusqlite::Connection;
use tokio::sync::{mpsc as async_mpsc, oneshot};

use super::{
    RunReport, TestTrigger,
    events::{CommandCall, Event, WebhookRequest},
    http::Http,
    sandbox::{self, Action, Environment, Identity, Outcome, Script, Sink, Storage, Triggers},
};
use crate::{
    db,
    error::{AppError, AppResult},
    now_ms, store,
};

pub enum Job {
    Event(Arc<Event>),
    Webhook(WebhookRequest, oneshot::Sender<sandbox::WebhookResponse>),
    Command(CommandCall, oneshot::Sender<CommandReply>),
    /// Runs the live script against a simulated event, for `run_automation`.
    Run(TestTrigger, oneshot::Sender<RunReport>),
}

/// What a command answered.
#[derive(Debug, Clone, Default)]
pub struct CommandReply {
    pub responses: Vec<String>,
    pub error: Option<String>,
}

/// What every worker of one generation shares.
#[derive(Clone)]
pub struct Shared {
    pub db_path: PathBuf,
    pub actions: async_mpsc::UnboundedSender<Action>,
    pub libraries: Arc<HashMap<String, String>>,
    pub secrets: Arc<BTreeMap<String, String>>,
    pub http: Arc<Http>,
    pub timezone: TimeZone,
}

/// Handle to a running automation.
pub struct Worker {
    pub jobs: mpsc::Sender<Job>,
    pub triggers: Arc<Mutex<Triggers>>,
    /// Whether the script loaded; failed scripts keep no thread.
    pub loaded: bool,
}

/// Starts a worker and waits until its script has loaded.
pub fn start(automation: &store::Automation, shared: &Shared) -> AppResult<Worker> {
    let (jobs, receiver) = mpsc::channel();
    let triggers = Arc::new(Mutex::new(Triggers::default()));
    let (loaded_tx, loaded_rx) = mpsc::channel();
    let automation = automation.clone();
    let shared = shared.clone();
    let worker_triggers = Arc::clone(&triggers);
    let id = automation.id;
    thread::Builder::new()
        .name(format!("sideporch-automation-{id}"))
        .spawn(move || {
            run(
                &automation,
                &shared,
                &receiver,
                &worker_triggers,
                &loaded_tx,
            );
        })
        .map_err(AppError::internal)?;
    // A script that never finishes loading is stopped by its budget.
    let loaded = loaded_rx
        .recv_timeout(Duration::from_secs(15))
        .unwrap_or(false);
    Ok(Worker {
        jobs,
        triggers,
        loaded,
    })
}

fn run(
    automation: &store::Automation,
    shared: &Shared,
    jobs: &mpsc::Receiver<Job>,
    triggers: &Arc<Mutex<Triggers>>,
    loaded: &mpsc::Sender<bool>,
) {
    let conn = match db::connect(&shared.db_path) {
        Ok(conn) => Arc::new(Mutex::new(conn)),
        Err(error) => {
            tracing::error!(?error, "automation could not open the database");
            let _ = loaded.send(false);
            return;
        }
    };
    let mut recorder = Recorder {
        conn: Arc::clone(&conn),
        id: automation.id,
        failing: automation.last_error.is_some(),
        secrets: Arc::clone(&shared.secrets),
    };
    let environment = Environment {
        identity: Identity {
            id: automation.id,
            name: automation.name.clone(),
        },
        storage: Storage::Live(conn),
        sink: Sink::Live(shared.actions.clone()),
        libraries: Arc::clone(&shared.libraries),
        secrets: Arc::clone(&shared.secrets),
        http: Some(Arc::clone(&shared.http)),
        timezone: shared.timezone.clone(),
    };
    let (script, outcome) = sandbox::load(&environment, &automation.source);
    recorder.record("load", outcome, false);
    let Some(mut script) = script else {
        let _ = loaded.send(false);
        return;
    };
    if let Ok(mut shown) = triggers.lock() {
        *shown = script.triggers();
    }
    let _ = loaded.send(true);
    loop {
        let wait = script
            .timers
            .iter()
            .filter_map(|timer| timer.next)
            .min()
            .map_or(Duration::from_hours(1), |next| {
                Duration::try_from(next.duration_since(Timestamp::now())).unwrap_or_default()
            });
        match jobs.recv_timeout(wait) {
            Ok(job) => handle(&script, job, &mut recorder),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        fire_timers(&mut script, &mut recorder, triggers);
    }
}

fn handle(script: &Script, job: Job, recorder: &mut Recorder) {
    match job {
        Job::Event(event) => {
            for outcome in sandbox::dispatch(script, &event) {
                recorder.record(event.kind().name(), outcome, false);
            }
        }
        Job::Webhook(request, respond) => {
            let response = match &script.webhook {
                None => {
                    sandbox::WebhookResponse::text(404, "This automation has no webhook handler.")
                }
                Some(handler) => {
                    let (mut outcome, response) =
                        sandbox::call_webhook(&script.lua, handler, &request);
                    let path = if request.path.is_empty() {
                        "/"
                    } else {
                        &request.path
                    };
                    outcome.log.insert(0, format!("{} {path}", request.method));
                    recorder.record("webhook", outcome, true);
                    response.unwrap_or_else(|| {
                        sandbox::WebhookResponse::text(500, "The automation failed.")
                    })
                }
            };
            // The caller may have timed out.
            let _ = respond.send(response);
        }
        Job::Command(command, respond) => {
            let reply = match sandbox::run_command(script, &command) {
                Some(mut outcome) => {
                    outcome.log.insert(
                        0,
                        format!(
                            "/{} {} by @{}",
                            command.name, command.text, command.username
                        ),
                    );
                    let reply = CommandReply {
                        responses: outcome.responses.clone(),
                        error: outcome.error.clone(),
                    };
                    recorder.record("command", outcome, true);
                    reply
                }
                None => CommandReply {
                    error: Some(format!("/{} is no longer registered", command.name)),
                    ..CommandReply::default()
                },
            };
            let _ = respond.send(reply);
        }
        Job::Run(trigger, respond) => {
            let (mut outcome, response, called) = super::simulate(script, &trigger);
            outcome.log.insert(0, "run by an agent over MCP".to_owned());
            let report = RunReport {
                ok: outcome.error.is_none(),
                error: outcome.error.clone(),
                log: outcome.log.clone(),
                responses: outcome.responses.clone(),
                called,
                response,
            };
            recorder.record("run", outcome, true);
            let _ = respond.send(report);
        }
    }
}

fn fire_timers(script: &mut Script, recorder: &mut Recorder, triggers: &Arc<Mutex<Triggers>>) {
    let now = Timestamp::now();
    let mut fired = false;
    for timer in &mut script.timers {
        if timer.next.is_some_and(|next| next <= now) {
            fired = true;
            timer.next = timer.schedule.next_after(now);
            let outcome = sandbox::call(&script.lua, &timer.callback, ());
            let trigger = match timer.schedule {
                sandbox::Schedule::Every(_) => "timer",
                sandbox::Schedule::Cron(..) => "cron",
            };
            recorder.record(trigger, outcome, false);
        }
    }
    if fired && let Ok(mut shown) = triggers.lock() {
        for (shown, timer) in shown.schedules.iter_mut().zip(&script.timers) {
            shown.next_run = timer.next.map(Timestamp::as_millisecond);
        }
    }
}

/// Keeps an automation's status and run log.
struct Recorder {
    conn: Arc<Mutex<Connection>>,
    id: i64,
    /// Whether the last call failed, so quiet successes only write when
    /// that changes.
    failing: bool,
    secrets: Arc<BTreeMap<String, String>>,
}

impl Recorder {
    /// Stores the status, and the call in the run log if it printed, acted,
    /// failed, or `always`.
    fn record(&mut self, trigger: &str, mut outcome: Outcome, always: bool) {
        let failed = outcome.error.is_some();
        let changed = failed != self.failing;
        self.failing = failed;
        let quiet = outcome.log.is_empty() && !failed && !always;
        if quiet && !changed {
            return;
        }
        redact(&mut outcome, &self.secrets);
        if let Some(error) = &outcome.error {
            tracing::warn!(automation = self.id, trigger, %error, "automation failed");
        }
        let Ok(conn) = self.conn.lock() else {
            return;
        };
        let result = store::set_automation_error(&conn, self.id, outcome.error.as_deref())
            .and_then(|()| {
                if quiet {
                    return Ok(());
                }
                let duration = i64::try_from(outcome.duration.as_micros()).unwrap_or(i64::MAX);
                store::record_automation_run(
                    &conn,
                    self.id,
                    trigger,
                    now_ms(),
                    duration,
                    &outcome.log.join("\n"),
                    outcome.error.as_deref(),
                )
            });
        if let Err(error) = result {
            tracing::warn!(?error, "could not record automation run");
        }
    }
}

/// Replaces secret values in logs and errors, so they never reach the run
/// log, the editor, or an agent.
pub fn redact(outcome: &mut Outcome, secrets: &BTreeMap<String, String>) {
    let hide = |text: &mut String| {
        for (name, value) in secrets {
            if value.len() >= 4 && text.contains(value.as_str()) {
                *text = text.replace(value.as_str(), &format!("[secret {name}]"));
            }
        }
    };
    outcome.log.iter_mut().for_each(hide);
    outcome.responses.iter_mut().for_each(hide);
    if let Some(error) = &mut outcome.error {
        hide(error);
    }
}
