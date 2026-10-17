//! App settings: the sync server address, with the default built in from `.env`.

use hostly_core::sync::ApiClient;
use serde::Serialize;
use tauri::State;

use crate::commands::{CommandError, CommandResult};
use crate::state::AppState;

/// `HOSTLY_SERVER_URL` at build time (see `build.rs`), if it was set.
const DEFAULT_SYNC_SERVER: Option<&str> = option_env!("HOSTLY_DEFAULT_SYNC_SERVER");

fn default_sync_server() -> Option<String> {
    DEFAULT_SYNC_SERVER.and_then(|server| ApiClient::normalize_server(server).ok())
}

/// The server chosen in Settings, if any. The only place the settings lock is
/// read: `std::sync::Mutex` isn't reentrant, and a nested lock on the main
/// thread freezes the whole app.
fn chosen_sync_server(state: &AppState) -> Option<String> {
    state
        .settings
        .lock()
        .unwrap()
        .settings()
        .sync_server
        .clone()
}

/// The server to log in to: the one chosen in Settings, else the built-in default.
pub(crate) fn sync_server(state: &AppState) -> Option<String> {
    chosen_sync_server(state).or_else(default_sync_server)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    /// Chosen in Settings; `None` means the default is used.
    sync_server: Option<String>,
    default_sync_server: Option<String>,
    /// What logging in uses.
    effective_sync_server: Option<String>,
}

fn view(state: &AppState) -> SettingsView {
    let chosen = chosen_sync_server(state);
    SettingsView {
        effective_sync_server: chosen.clone().or_else(default_sync_server),
        sync_server: chosen,
        default_sync_server: default_sync_server(),
    }
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> SettingsView {
    view(&state)
}

/// `None` or blank returns to the default. Refused while logged in, since the
/// login belongs to the current server.
#[tauri::command]
pub fn set_sync_server(
    state: State<'_, AppState>,
    server: Option<String>,
) -> CommandResult<SettingsView> {
    if let Some(login) = state.sync.file.lock().unwrap().login() {
        return Err(CommandError::new(
            "loggedIn",
            format!(
                "You're logged in to {}. Log out to switch servers.",
                login.server
            ),
        ));
    }
    state
        .settings
        .lock()
        .unwrap()
        .set_sync_server(server.as_deref())?;
    Ok(view(&state))
}

/// Checks that `server` is reachable and is a Hostly sync server.
#[tauri::command]
pub async fn check_sync_server(server: String) -> CommandResult<()> {
    Ok(ApiClient::new(&server, None)?.health().await?)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use hostly_core::secrets::MemoryStore;

    use super::*;

    fn state(dir: &Path) -> AppState {
        AppState::open(dir, Arc::new(MemoryStore::default()), None).unwrap()
    }

    /// `get_settings` runs on the main thread, so a lock it never gets back hangs the app.
    #[test]
    fn the_settings_view_returns_instead_of_deadlocking() {
        let dir = tempfile::tempdir().unwrap();
        let state = state(dir.path());
        state
            .settings
            .lock()
            .unwrap()
            .set_sync_server(Some("https://sync.example.com"))
            .unwrap();

        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || done.send(view(&state)).unwrap());
        let view = finished
            .recv_timeout(Duration::from_secs(5))
            .expect("the settings view deadlocked");
        assert_eq!(
            view.sync_server.as_deref(),
            Some("https://sync.example.com")
        );
        assert_eq!(
            view.effective_sync_server.as_deref(),
            Some("https://sync.example.com")
        );
    }
}
