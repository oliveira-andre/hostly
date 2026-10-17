//! Accounts on a Hostly sync server, and the background loop that keeps this
//! device's hosts, snippets, port forwards and secrets in sync with the
//! account. Sync rules live in `hostly_core::sync`; encryption in
//! `hostly_core::vault`.

use std::sync::Mutex;
use std::time::Duration;

use hostly_core::Error;
use hostly_core::sync::{
    self, ApiClient, ItemKey, LoggedIn, Login, SESSION_SECRET_KEY, SyncFile, secrets,
};
use hostly_core::vault::{VAULT_KEY_SECRET_KEY, VaultKey};
use hostly_protocol::{Account, Kind, error_code};
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

/// This device's copy of the account's vault key, read from the keychain at
/// most once per run.
#[derive(Clone)]
enum Vault {
    NotRead,
    Missing,
    Present(VaultKey),
}

pub struct SyncService {
    pub file: Mutex<SyncFile>,
    /// The session token, read from the keychain at most once per run.
    token: Mutex<Option<String>>,
    vault: Mutex<Vault>,
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
            vault: Mutex::new(Vault::NotRead),
            status: Mutex::new(status),
            wake: Notify::new(),
        }
    }

    /// Notes an item saved or deleted on this device and syncs it soon.
    /// Recorded while logged out too, so the change keeps its timestamp for a
    /// later merge.
    pub fn record(&self, kind: Kind, id: Uuid, deleted: bool) -> hostly_core::Result<()> {
        self.file.lock().unwrap().record_local_change(
            ItemKey::new(kind, id),
            deleted,
            sync::now_ms(),
        )?;
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
    /// Whether this device holds the vault key, so passwords sync. A device
    /// logged in before end-to-end encryption needs the password once.
    password_sync: bool,
}

#[tauri::command]
pub async fn account_status(state: State<'_, AppState>) -> CommandResult<AccountStatus> {
    let login = state.sync.file.lock().unwrap().login().cloned();
    let password_sync = login.is_some() && vault_key(&state).await?.is_some();
    Ok(AccountStatus {
        server: login.as_ref().map(|l| l.server.clone()),
        account: login.map(|l| l.account),
        sync: state.sync.status.lock().unwrap().clone(),
        password_sync,
    })
}

/// Creates an end-to-end encrypted account on the sync server from Settings
/// and logs in. The password never leaves this device.
#[tauri::command]
pub async fn sign_up(
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> CommandResult<Account> {
    let server = settings::sync_server(&state).ok_or(Error::NoSyncServer)?;
    let logged_in = ApiClient::new(&server, None)?
        .create_account(&email, &password)
        .await?;
    start_session(&app, &state, server, logged_in).await
}

/// Logs in to the sync server from Settings, upgrading an account created by
/// an older app to end-to-end encryption. Also turns on password sync for a
/// device that is already logged in.
#[tauri::command]
pub async fn log_in(
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
    password: String,
) -> CommandResult<Account> {
    let server = settings::sync_server(&state).ok_or(Error::NoSyncServer)?;
    let logged_in = ApiClient::new(&server, None)?
        .open_account(&email, &password)
        .await?;
    start_session(&app, &state, server, logged_in).await
}

/// Turns on password sync on a device logged in before end-to-end encryption,
/// by logging in again to the same account with its password.
#[tauri::command]
pub async fn unlock_password_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    password: String,
) -> CommandResult<Account> {
    let login = state
        .sync
        .file
        .lock()
        .unwrap()
        .login()
        .cloned()
        .ok_or(Error::NotLoggedIn)?;
    let logged_in = ApiClient::new(&login.server, None)?
        .open_account(&login.account.email, &password)
        .await?;
    start_session(&app, &state, login.server, logged_in).await
}

/// Changes the account password. Saved passwords stay readable everywhere:
/// only the wrapping of the vault key changes, and other devices stay logged in.
#[tauri::command]
pub async fn change_password(
    state: State<'_, AppState>,
    current: String,
    new: String,
) -> CommandResult<()> {
    let login = state
        .sync
        .file
        .lock()
        .unwrap()
        .login()
        .cloned()
        .ok_or(Error::NotLoggedIn)?;
    let vault = vault_key(&state).await?.ok_or_else(|| {
        Error::Vault("turn on password sync on this device before changing the password".into())
    })?;
    let token = session_token(&state).await?.ok_or(Error::NotLoggedIn)?;
    ApiClient::new(&login.server, Some(token))?
        .change_password(&login.account.email, &current, &new, &vault)
        .await?;
    Ok(())
}

/// Stores the new session and vault key, then syncs. A new login always
/// starts with a first merge, which brings in secrets this device couldn't
/// read before.
async fn start_session(
    app: &AppHandle,
    state: &AppState,
    server: String,
    logged_in: LoggedIn,
) -> CommandResult<Account> {
    let LoggedIn { session, vault } = logged_in;
    let previous = state.sync.token.lock().unwrap().clone();
    let secrets = state.secrets.clone();
    let (token, vault_text) = (session.token.clone(), vault.to_base64());
    blocking(move || {
        secrets.set(SESSION_SECRET_KEY, &token)?;
        secrets.set(VAULT_KEY_SECRET_KEY, &vault_text)
    })
    .await?;
    *state.sync.token.lock().unwrap() = Some(session.token);
    *state.sync.vault.lock().unwrap() = Vault::Present(vault);
    state
        .sync
        .file
        .lock()
        .unwrap()
        .set_login(Some(Login::new(server.clone(), session.account.clone())))?;
    // Logging in again replaces this device's earlier session.
    if let Some(previous) = previous
        && let Ok(api) = ApiClient::new(&server, Some(previous))
    {
        let _ = api.log_out().await;
    }
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

/// Stops syncing. Hosts, snippets, port forwards and saved passwords stay on
/// this device.
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
    *state.sync.vault.lock().unwrap() = Vault::Missing;
    let secrets = state.secrets.clone();
    blocking(move || {
        secrets.delete(SESSION_SECRET_KEY)?;
        secrets.delete(VAULT_KEY_SECRET_KEY)
    })
    .await?;
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
    let fail = |message: String| SyncStatus {
        state: "error",
        last_synced_at,
        message: Some(message),
    };
    let token = match session_token(state).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            let _ = forget_session(app, state, Some("Your session ended. Log in again.")).await;
            return;
        }
        Err(error) => {
            state.sync.set_status(app, fail(error.to_string()));
            return;
        }
    };
    let vault = match vault_key(state).await {
        Ok(vault) => vault,
        Err(error) => {
            state.sync.set_status(app, fail(error.to_string()));
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
    let result = run_round(app, state, &login, token, vault).await;
    match result {
        Ok(report) => {
            for (changed, event) in [
                (report.hosts_changed, "hosts-changed"),
                (report.snippets_changed, "snippets-changed"),
                (report.forwards_changed, "forwards-changed"),
            ] {
                if changed {
                    let _ = app.emit(event, ());
                }
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
        Err(error) => state.sync.set_status(app, fail(error.to_string())),
    }
}

/// Seals the secrets the round needs, syncs, then writes received secrets to
/// the keychain. Keychain calls run off the async workers.
async fn run_round(
    app: &AppHandle,
    state: &AppState,
    login: &Login,
    token: String,
    vault: Option<VaultKey>,
) -> hostly_core::Result<sync::SyncReport> {
    let api = ApiClient::new(&login.server, Some(token))?;
    let sealed = match &vault {
        Some(vault) => {
            let ids = secrets::wanted(&state.hosts, &state.sync.file);
            let (keychain, vault) = (state.secrets.clone(), vault.clone());
            let sealed = tauri::async_runtime::spawn_blocking(move || {
                secrets::seal(&ids, keychain.as_ref(), &vault)
            })
            .await
            .map_err(|e| Error::Keychain(e.to_string()))?;
            Some(sealed)
        }
        None => None,
    };
    let report = sync::sync_once(
        &state.local(),
        &state.sync.file,
        &api,
        sealed.as_ref(),
        sync::now_ms(),
    )
    .await?;
    if let Some(vault) = vault {
        let _writing = state.secret_writes.lock().await;
        let app = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let state = app.state::<AppState>();
            secrets::deliver(&state.sync.file, state.secrets.as_ref(), &vault)
        })
        .await
        .map_err(|e| Error::Keychain(e.to_string()))??;
    }
    Ok(report)
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

/// The cached vault key, or the keychain's. `None` until this device logs in
/// with end-to-end encryption.
async fn vault_key(state: &AppState) -> hostly_core::Result<Option<VaultKey>> {
    let cached = state.sync.vault.lock().unwrap().clone();
    match cached {
        Vault::Present(vault) => return Ok(Some(vault)),
        Vault::Missing => return Ok(None),
        Vault::NotRead => {}
    }
    let secrets = state.secrets.clone();
    let stored = tauri::async_runtime::spawn_blocking(move || secrets.get(VAULT_KEY_SECRET_KEY))
        .await
        .map_err(|e| Error::Keychain(e.to_string()))??;
    let vault = match stored.as_deref().map(VaultKey::from_base64) {
        Some(Ok(vault)) => Vault::Present(vault),
        _ => Vault::Missing,
    };
    *state.sync.vault.lock().unwrap() = vault.clone();
    Ok(match vault {
        Vault::Present(vault) => Some(vault),
        _ => None,
    })
}
