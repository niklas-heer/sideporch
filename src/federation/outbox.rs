//! Delivering events to other servers, in order, until they take them.

use std::time::Duration;

use super::{
    data::{self, Status},
    events::{Batch, Event},
};
use crate::{AppState, error::AppResult, now_ms};

/// Events per request.
const BATCH: i64 = 50;
/// How often to look for events due again, besides being woken.
const TICK: Duration = Duration::from_secs(5);

/// Delivers queued events in the background.
pub fn start(state: AppState) {
    tokio::spawn(async move {
        loop {
            // A permit from notify_one() before this waits wakes it at once.
            drop(tokio::time::timeout(TICK, state.federation.wake.notified()).await);
            if let Err(error) = deliver_due(&state).await {
                tracing::warn!(%error, "delivering to other servers");
            }
        }
    });
}

/// Sends everything that's due, server by server.
pub async fn deliver_due(state: &AppState) -> AppResult<()> {
    let due = state.db.call(|conn| data::due(conn, now_ms())).await?;
    for instance_id in due {
        // Keep sending while this server takes whole batches.
        while deliver_batch(state, instance_id).await? {}
    }
    Ok(())
}

/// Sends one batch to `instance_id`. Returns whether more may wait.
async fn deliver_batch(state: &AppState, instance_id: i64) -> AppResult<bool> {
    let (instance, waiting) = state
        .db
        .call(move |conn| {
            Ok((
                data::instance(conn, instance_id)?,
                data::waiting(conn, instance_id, BATCH)?,
            ))
        })
        .await?;
    let Some(instance) = instance.filter(|instance| instance.status == Status::Connected) else {
        // Nothing goes to servers that aren't connected (any more).
        let ids: Vec<i64> = waiting.iter().map(|(id, _)| *id).collect();
        state
            .db
            .call(move |conn| data::delivered(conn, &ids))
            .await?;
        return Ok(false);
    };
    if waiting.is_empty() {
        return Ok(false);
    }
    let ids: Vec<i64> = waiting.iter().map(|(id, _)| *id).collect();
    let events: Vec<Event> = waiting
        .iter()
        .filter_map(|(_, json)| serde_json::from_str(json).ok())
        .collect();
    let full = i64::try_from(ids.len()).unwrap_or(0) >= BATCH;
    let body = serde_json::to_vec(&Batch { events }).map_err(crate::error::AppError::internal)?;
    let sent = state
        .federation
        .send("POST", &instance.url, "/federation/inbox", body)
        .await;
    match sent {
        Ok((status, _)) if (200..300).contains(&status) => {
            state
                .db
                .call(move |conn| data::delivered(conn, &ids))
                .await?;
            Ok(full)
        }
        outcome => {
            let error = match outcome {
                Ok((status, _)) => format!("{} answered {status}", instance.handle),
                Err(error) => error,
            };
            tracing::warn!(server = %instance.handle, %error, "could not deliver to another server; trying again later");
            let now = now_ms();
            state
                .db
                .call(move |conn| data::failed(conn, instance_id, &error, now))
                .await?;
            Ok(false)
        }
    }
}
