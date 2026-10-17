//! Running saved port-forwarding rules. Each running rule holds its own SSH
//! connection, and reports its state to the UI through `forward-status` events.

use hostly_core::Error;
use hostly_core::forward::{ForwardEvent, ForwardHandle, ForwardSpec};
use hostly_core::ssh;
use hostly_protocol::{Forward, ForwardKind};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

use crate::commands::{CommandResult, connect_request};
use crate::state::AppState;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForwardStatus {
    id: Uuid,
    /// "starting", "running", "stopped" or "error".
    state: &'static str,
    message: Option<String>,
    /// Tunneled connections open right now.
    connections: usize,
}

/// A rule that is starting, running, or stopped with an error. `run` tells one
/// start from the next, so a stopped run's late events change nothing.
pub struct Run {
    run: Uuid,
    handle: Option<ForwardHandle>,
    status: ForwardStatus,
}

fn status(id: Uuid, state: &'static str, message: Option<String>) -> ForwardStatus {
    ForwardStatus {
        id,
        state,
        message,
        connections: 0,
    }
}

fn spec(rule: &Forward) -> ForwardSpec {
    let (bind_address, bind_port) = (rule.bind_address.clone(), rule.bind_port);
    let destination_host = rule.destination_host.clone().unwrap_or_default();
    let destination_port = rule.destination_port.unwrap_or_default();
    match rule.kind {
        ForwardKind::Local => ForwardSpec::Local {
            bind_address,
            bind_port,
            destination_host,
            destination_port,
        },
        ForwardKind::Remote => ForwardSpec::Remote {
            bind_address,
            bind_port,
            destination_host,
            destination_port,
        },
        ForwardKind::Dynamic => ForwardSpec::Dynamic {
            bind_address,
            bind_port,
        },
    }
}

/// Applies `change` to the rule's run if it is still `run`, and tells the UI.
fn update(app: &AppHandle, id: Uuid, run: Uuid, change: impl FnOnce(&mut Option<Run>)) {
    let state = app.state::<AppState>();
    let mut runs = state.forwarding.lock().unwrap();
    if runs.get(&id).is_some_and(|entry| entry.run != run) {
        return;
    }
    let mut entry = runs.remove(&id);
    change(&mut entry);
    let shown = entry
        .as_ref()
        .map_or_else(|| status(id, "stopped", None), |e| e.status.clone());
    if let Some(entry) = entry {
        runs.insert(id, entry);
    }
    drop(runs);
    let _ = app.emit("forward-status", shown);
}

/// Connects to the rule's host and starts listening. Resolves once the forward
/// is listening; `secret` overrides the host's saved password or passphrase.
#[tauri::command]
pub async fn start_forward(
    app: AppHandle,
    state: State<'_, AppState>,
    id: Uuid,
    secret: Option<String>,
) -> CommandResult<()> {
    let rule = state
        .forwards
        .lock()
        .unwrap()
        .get(id)
        .cloned()
        .ok_or(Error::NotFound {
            kind: "port forward",
            id,
        })?;
    let run = Uuid::new_v4();
    {
        let mut runs = state.forwarding.lock().unwrap();
        if runs.get(&id).is_some_and(|entry| entry.handle.is_some()) {
            return Ok(());
        }
        runs.insert(
            id,
            Run {
                run,
                handle: None,
                status: status(id, "starting", None),
            },
        );
    }
    let _ = app.emit("forward-status", status(id, "starting", None));

    let events = app.clone();
    let started = async {
        let request = connect_request(&state, rule.host_id, secret).await?;
        let connection = ssh::connect(request).await?;
        let handle = connection
            .start_forward(spec(&rule), move |event| on_event(&events, id, run, event))
            .await?;
        Ok::<_, crate::commands::CommandError>(handle)
    }
    .await;
    match started {
        Ok(handle) => {
            let mut keep = None;
            update(&app, id, run, |entry| {
                // Stopped while connecting: the new forward isn't wanted any more.
                if let Some(entry) = entry.as_mut().filter(|e| e.status.state == "starting") {
                    entry.handle = Some(handle.clone());
                    entry.status.state = "running";
                    keep = Some(());
                }
            });
            if keep.is_none() {
                handle.stop();
            }
            Ok(())
        }
        Err(error) => {
            let message = error.message().to_owned();
            update(&app, id, run, |entry| {
                if let Some(entry) = entry {
                    entry.status = status(id, "error", Some(message));
                }
            });
            Err(error)
        }
    }
}

fn on_event(app: &AppHandle, id: Uuid, run: Uuid, event: ForwardEvent) {
    match event {
        ForwardEvent::Connections { active } => update(app, id, run, |entry| {
            if let Some(entry) = entry {
                entry.status.connections = active;
            }
        }),
        ForwardEvent::ConnectionFailed { .. } => {}
        // A clean stop forgets the rule; a dropped connection keeps the reason.
        ForwardEvent::Stopped { error } => update(app, id, run, |entry| {
            *entry = match (entry.take(), error) {
                (Some(previous), Some(error)) => Some(Run {
                    run: previous.run,
                    handle: None,
                    status: status(id, "error", Some(error)),
                }),
                _ => None,
            };
        }),
    }
}

#[tauri::command]
pub fn stop_forward(app: AppHandle, state: State<'_, AppState>, id: Uuid) {
    stop(&app, &state, id);
}

/// Stops the rule if it is running, and forgets any error it stopped with.
pub fn stop(app: &AppHandle, state: &AppState, id: Uuid) {
    let entry = state.forwarding.lock().unwrap().remove(&id);
    if let Some(handle) = entry.and_then(|entry| entry.handle) {
        handle.stop();
    }
    let _ = app.emit("forward-status", status(id, "stopped", None));
}

/// Rules that are starting, running or stopped with an error; others are stopped.
#[tauri::command]
pub fn forward_statuses(state: State<'_, AppState>) -> Vec<ForwardStatus> {
    let runs = state.forwarding.lock().unwrap();
    runs.values().map(|entry| entry.status.clone()).collect()
}
