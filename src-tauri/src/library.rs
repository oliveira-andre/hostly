//! Snippets and port-forwarding rules: saved like hosts, and synced with the
//! account. Running a rule lives in `forwarding.rs`.

use hostly_protocol::{Forward, Kind, Snippet};
use tauri::{AppHandle, State};
use uuid::Uuid;

use crate::commands::CommandResult;
use crate::state::AppState;

#[tauri::command]
pub fn list_snippets(state: State<'_, AppState>) -> Vec<Snippet> {
    state.snippets.lock().unwrap().list().to_vec()
}

#[tauri::command]
pub fn save_snippet(state: State<'_, AppState>, snippet: Snippet) -> CommandResult<Snippet> {
    let saved = state.snippets.lock().unwrap().upsert(snippet)?;
    state.sync.record(Kind::Snippet, saved.id, false)?;
    Ok(saved)
}

#[tauri::command]
pub fn delete_snippet(state: State<'_, AppState>, id: Uuid) -> CommandResult<()> {
    state.snippets.lock().unwrap().remove(id)?;
    Ok(state.sync.record(Kind::Snippet, id, true)?)
}

#[tauri::command]
pub fn list_forwards(state: State<'_, AppState>) -> Vec<Forward> {
    state.forwards.lock().unwrap().list().to_vec()
}

#[tauri::command]
pub fn save_forward(state: State<'_, AppState>, forward: Forward) -> CommandResult<Forward> {
    let saved = state.forwards.lock().unwrap().upsert(forward)?;
    state.sync.record(Kind::Forward, saved.id, false)?;
    Ok(saved)
}

/// Stops the rule first if it is running.
#[tauri::command]
pub fn delete_forward(app: AppHandle, state: State<'_, AppState>, id: Uuid) -> CommandResult<()> {
    crate::forwarding::stop(&app, &state, id);
    state.forwards.lock().unwrap().remove(id)?;
    Ok(state.sync.record(Kind::Forward, id, true)?)
}
