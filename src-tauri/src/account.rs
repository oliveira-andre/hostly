//! Accounts on a Hostly sync server, and the background loop that keeps this
//! device's hosts in sync with the account. Sync rules live in `hostly_core::sync`.

use std::sync::Mutex;
use std::time::Duration;

use hostly_core::Error;
use hostly_core::sync::{self, ApiClient, Login, SESSION_SECRET_KEY, SyncFile};
use hostly_protocol::{Account, Credentials, error_code};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;
use uuid::Uuid;

use crate::commands::{CommandResult, blocking};
use crate::settings;
use crate::state::AppState;

/// How often to look for changes made on other devices.
const SYNC_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// "loggedOut", "syncing", "synced" or "error".
    state: &'static str,
    last_synced_at: Option<i64>,
    message: Option<String>,
}

impl SyncStatus {
    fn logged_out(message: Option<&str>) -> Self {
        Self {
            state: "loggedOut",
            last_synced_at: None,
            message: message.map(Into::into),
        }
    }
}

pub struct SyncService {
    pub file: Mutex<SyncFile>,
    /// The session token, read from the keychain at most once per run.
    token: Mutex<Option<String>>,
    status: Mutex<SyncStatus>,
    wake: Notify,
}

impl SyncService {
    pub fn new(file: SyncFile) -> Self {
        let status = match file.login() {
            Some(_) => SyncStatus {
                state: "syncing",
                last_synced_at: None,
                message: None,
            },
            None => SyncStatus::logged_out(None),
        };
        Self {
            file: Mutex::new(file),
            token: Mutex::default(),
            status: Mutex::new(status),
            wake: Notify::new(),
        }
    }

    /// Notes a host saved or deleted on this device and syncs it soon. Recorded
    /// while logged out too, so the change keeps its timestamp for a later merge.
    pub fn record(&self, id: Uuid, deleted: bool) -> hostly_core::Result<()> {
        self.file
            .lock()
            .unwrap()
            .record_local_change(id, deleted, sync::now_ms())?;
        self.wake.notify_one();
        Ok(())
    }

    fn set_status(&self, app: &AppHandle, status: SyncStatus) {
        *self.status.lock().unwrap() = status.clone();
        let _ = app.emit("sync-status", status);
    }

    fn last_synced_at(&self) -> Option<i64> {
        self.status.lock().unwrap().last_synced_at
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    server: Option<String>,
    account: Option<Account>,
    sync: SyncStatus,
}

#[tauri::command]
pub fn account_status(state: State<'_, AppState>) -> AccountStatus {
    let login = state.sync.file.lock().unwrap().login().cloned();
    AccountStatus {
        server: login.as_ref().map(|l| l.server.clone()),
        account: login.map(|l| l.account),
        sync: state.sync.status.lock().unwrap().clone(),
    }
}

/// Creates an account on the sync server from Settings and logs in.
#[tauri::command]
pub async fn sign_up(
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> CommandResult<Account> {
    start_session(&app, &state, Credentials { email, password }, true).await
}

/// Logs in to the sync server from Settings.
#[tauri::command]
pub async fn log_in(
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> CommandResult<Account> {
    start_session(&app, &state, Credentials { email, password }, false).await
}

async fn start_session(
    app: &AppHandle,
    state: &AppState,
    credentials: Credentials,
    create: bool,
) -> CommandResult<Account> {
    let server = settings::sync_server(state).ok_or(Error::NoSyncServer)?;
    let api = ApiClient::new(&server, None)?;
    let session = if create {
        api.sign_up(&credentials).await?
    } else {
        api.log_in(&credentials).await?
    };

    let secrets = state.secrets.clone();
    let token = session.token.clone();
    blocking(move || secrets.set(SESSION_SECRET_KEY, &token)).await?;
    *state.sync.token.lock().unwrap() = Some(session.token);
    state
        .sync
        .file
        .lock()
        .unwrap()
        .set_login(Some(Login::new(server, session.account.clone())))?;
    state.sync.set_status(
        app,
        SyncStatus {
            state: "syncing",
            last_synced_at: None,
            message: None,
        },
    );
    state.sync.wake.notify_one();
    Ok(session.account)
}

/// Stops syncing. Hosts stay on this device.
#[tauri::command]
pub async fn log_out(app: AppHandle, state: State<'_, AppState>) -> CommandResult<()> {
    let login = state.sync.file.lock().unwrap().login().cloned();
    let token = state.sync.token.lock().unwrap().take();
    if let (Some(login), Some(token)) = (login, token) {
        // Best effort: the session is forgotten here even if the server is unreachable.
        if let Ok(api) = ApiClient::new(&login.server, Some(token)) {
            let _ = api.log_out().await;
        }
    }
    forget_session(&app, &state, None).await
}

async fn forget_session(
    app: &AppHandle,
    state: &AppState,
    message: Option<&str>,
) -> CommandResult<()> {
    state.sync.file.lock().unwrap().set_login(None)?;
    *state.sync.token.lock().unwrap() = None;
    let secrets = state.secrets.clone();
    blocking(move || secrets.delete(SESSION_SECRET_KEY)).await?;
    state.sync.set_status(app, SyncStatus::logged_out(message));
    Ok(())
}

#[tauri::command]
pub fn sync_now(state: State<'_, AppState>) {
    state.sync.wake.notify_one();
}

/// Syncs every [`SYNC_INTERVAL`], and right away after local changes,
/// logging in, or [`sync_now`].
pub fn spawn_sync_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(SYNC_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let state = app.state::<AppState>();
            tokio::select! {
                _ = interval.tick() => {}
                _ = state.sync.wake.notified() => {}
            }
            sync_round(&app, &state).await;
        }
    });
}

async fn sync_round(app: &AppHandle, state: &AppState) {
    let Some(login) = state.sync.file.lock().unwrap().login().cloned() else {
        return;
    };
    let last_synced_at = state.sync.last_synced_at();
    let token = match session_token(state).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            let _ = forget_session(app, state, Some("Your session ended. Log in again.")).await;
            return;
        }
        Err(error) => {
            state.sync.set_status(
                app,
                SyncStatus {
                    state: "error",
                    last_synced_at,
                    message: Some(error.to_string()),
                },
            );
            return;
        }
    };

    state.sync.set_status(
        app,
        SyncStatus {
            state: "syncing",
            last_synced_at,
            message: None,
        },
    );
    let result = match ApiClient::new(&login.server, Some(token)) {
        Ok(api) => sync::sync_once(&state.hosts, &state.sync.file, &api, sync::now_ms()).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(report) => {
            if report.hosts_changed {
                let _ = app.emit("hosts-changed", ());
            }
            state.sync.set_status(
                app,
                SyncStatus {
                    state: "synced",
                    last_synced_at: Some(sync::now_ms()),
                    message: None,
                },
            );
        }
        // The token was revoked, e.g. by logging out on the server side.
        Err(Error::Api { code, .. }) if code == error_code::UNAUTHORIZED => {
            let _ = forget_session(app, state, Some("Your session ended. Log in again.")).await;
        }
        Err(error) => state.sync.set_status(
            app,
            SyncStatus {
                state: "error",
                last_synced_at,
                message: Some(error.to_string()),
            },
        ),
    }
}

/// The cached token, or the keychain's. `Ok(None)` means there is no session.
async fn session_token(state: &AppState) -> hostly_core::Result<Option<String>> {
    if let Some(token) = state.sync.token.lock().unwrap().clone() {
        return Ok(Some(token));
    }
    let secrets = state.secrets.clone();
    let token = tauri::async_runtime::spawn_blocking(move || secrets.get(SESSION_SECRET_KEY))
        .await
        .map_err(|e| Error::Keychain(e.to_string()))??;
    *state.sync.token.lock().unwrap() = token.clone();
    Ok(token)
}
