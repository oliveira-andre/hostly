use hostly_core::hosts::{AuthMethod, Host, expand_home};
use hostly_core::secrets::host_secret_key;
use hostly_core::ssh::{self, ConnectRequest, Credentials, SessionEvent, SessionHandle};
use hostly_core::{Error, HostKeyInfo};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::state::AppState;

/// What the UI receives when a command fails; `kind` drives its reaction.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommandError {
    kind: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_key: Option<Box<HostKeyInfo>>,
}

impl From<Error> for CommandError {
    fn from(error: Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
            host_key: error.host_key().cloned().map(Box::new),
        }
    }
}

impl CommandError {
    pub(crate) fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            host_key: None,
        }
    }
}

pub(crate) type CommandResult<T> = Result<T, CommandError>;

/// Keychain calls can block on a system permission dialog, so keep them off the async workers.
pub(crate) async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> hostly_core::Result<T> + Send + 'static,
) -> CommandResult<T> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|e| CommandError {
            kind: "internal",
            message: e.to_string(),
            host_key: None,
        })?
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_hosts(state: State<'_, AppState>) -> Vec<Host> {
    state.hosts.lock().unwrap().list().to_vec()
}

/// `secret`: omitted keeps the saved secret, empty deletes it, anything else replaces it.
#[tauri::command]
pub async fn save_host(
    state: State<'_, AppState>,
    host: Host,
    secret: Option<String>,
) -> CommandResult<Host> {
    let saved = state.hosts.lock().unwrap().upsert(host)?;
    state.sync.record(saved.id, false)?;
    if let Some(secret) = secret {
        let secrets = state.secrets.clone();
        let key = host_secret_key(saved.id);
        blocking(move || match secret.as_str() {
            "" => secrets.delete(&key),
            secret => secrets.set(&key, secret),
        })
        .await?;
    }
    Ok(saved)
}

#[tauri::command]
pub async fn delete_host(state: State<'_, AppState>, id: Uuid) -> CommandResult<()> {
    state.hosts.lock().unwrap().remove(id)?;
    state.sync.record(id, true)?;
    let secrets = state.secrets.clone();
    blocking(move || secrets.delete(&host_secret_key(id))).await
}

/// The saved password or key passphrase, so the owner can view and copy it.
#[tauri::command]
pub async fn get_host_secret(
    state: State<'_, AppState>,
    id: Uuid,
) -> CommandResult<Option<String>> {
    let secrets = state.secrets.clone();
    blocking(move || secrets.get(&host_secret_key(id))).await
}

/// Saves a password or passphrase entered at a connection prompt.
#[tauri::command]
pub async fn set_host_secret(
    state: State<'_, AppState>,
    id: Uuid,
    secret: String,
) -> CommandResult<()> {
    if state.hosts.lock().unwrap().get(id).is_none() {
        return Err(Error::HostNotFound(id).into());
    }
    let secrets = state.secrets.clone();
    blocking(move || secrets.set(&host_secret_key(id), &secret)).await
}

/// Records a host key the user approved after an `unknownHostKey` error.
#[tauri::command]
pub fn trust_host_key(
    state: State<'_, AppState>,
    host: String,
    port: u16,
    public_key: String,
) -> CommandResult<()> {
    Ok(state.known_hosts.trust_openssh(&host, port, &public_key)?)
}

#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SessionStatus {
    Closed {
        exit_status: Option<u32>,
        error: Option<String>,
    },
}

/// Builds the connection request for a saved host. `secret` overrides the
/// saved password or passphrase for this attempt.
pub(crate) async fn connect_request(
    state: &AppState,
    host_id: Uuid,
    secret: Option<String>,
) -> CommandResult<ConnectRequest> {
    let host = state
        .hosts
        .lock()
        .unwrap()
        .get(host_id)
        .cloned()
        .ok_or(Error::HostNotFound(host_id))?;

    let saved_secret = || {
        let secrets = state.secrets.clone();
        blocking(move || secrets.get(&host_secret_key(host_id)))
    };
    let credentials = match host.auth {
        AuthMethod::Password => {
            let password = match secret {
                Some(secret) => Some(secret),
                None => saved_secret().await?,
            };
            Credentials::Password(password.ok_or(Error::PasswordRequired)?)
        }
        AuthMethod::Key { path } => Credentials::Key {
            path: expand_home(&path, state.home_dir.as_deref()),
            passphrase: match secret {
                Some(secret) => Some(secret),
                None => saved_secret().await?,
            },
        },
        AuthMethod::Agent => Credentials::Agent,
    };

    Ok(ConnectRequest {
        host: host.hostname,
        port: host.port,
        username: host.username,
        credentials,
        known_hosts: state.known_hosts.clone(),
    })
}

/// Opens a shell on a saved host and returns its session id. Terminal output
/// streams through `on_data` as raw bytes; `on_status` reports the close.
/// `secret` overrides the saved password or passphrase for this attempt.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn connect(
    app: AppHandle,
    state: State<'_, AppState>,
    host_id: Uuid,
    cols: u32,
    rows: u32,
    secret: Option<String>,
    on_data: Channel,
    on_status: Channel<SessionStatus>,
) -> CommandResult<Uuid> {
    let request = connect_request(&state, host_id, secret).await?;
    let session_id = Uuid::new_v4();
    let handle = ssh::connect(request)
        .await?
        .open_shell(cols, rows, move |event| match event {
            SessionEvent::Data(bytes) => {
                let _ = on_data.send(InvokeResponseBody::Raw(bytes));
            }
            SessionEvent::Closed { exit_status, error } => {
                app.state::<AppState>()
                    .sessions
                    .lock()
                    .unwrap()
                    .remove(&session_id);
                let _ = on_status.send(SessionStatus::Closed { exit_status, error });
            }
        })
        .await?;
    state.sessions.lock().unwrap().insert(session_id, handle);
    Ok(session_id)
}

fn session(state: &AppState, id: Uuid) -> CommandResult<SessionHandle> {
    let handle = state.sessions.lock().unwrap().get(&id).cloned();
    Ok(handle.ok_or(Error::SessionClosed)?)
}

#[tauri::command]
pub fn write(state: State<'_, AppState>, session_id: Uuid, data: Vec<u8>) -> CommandResult<()> {
    Ok(session(&state, session_id)?.write(data)?)
}

#[tauri::command]
pub fn resize(
    state: State<'_, AppState>,
    session_id: Uuid,
    cols: u32,
    rows: u32,
) -> CommandResult<()> {
    Ok(session(&state, session_id)?.resize(cols, rows)?)
}

#[tauri::command]
pub fn disconnect(state: State<'_, AppState>, session_id: Uuid) {
    if let Some(handle) = state.sessions.lock().unwrap().remove(&session_id) {
        handle.close();
    }
}
