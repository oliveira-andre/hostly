//! Commands behind the SFTP browser: remote and local listings, and transfers.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use hostly_core::files::FileEntry;
use hostly_core::sftp::{SftpClient, TransferProgress};
use hostly_core::{Error, local_fs, ssh};
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;
use uuid::Uuid;

use crate::commands::{CommandResult, blocking, connect_request};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SftpOpened {
    sftp_id: Uuid,
    home: String,
}

/// Connects to a saved host and opens SFTP; fails with the same kinds as `connect`.
#[tauri::command]
pub async fn sftp_open(
    state: State<'_, AppState>,
    host_id: Uuid,
    secret: Option<String>,
) -> CommandResult<SftpOpened> {
    let request = connect_request(&state, host_id, secret).await?;
    let client = ssh::connect(request).await?.open_sftp().await?;
    let home = client.home().await?;
    let sftp_id = Uuid::new_v4();
    state.sftp.lock().unwrap().insert(sftp_id, Arc::new(client));
    Ok(SftpOpened { sftp_id, home })
}

fn client(state: &AppState, id: Uuid) -> CommandResult<Arc<SftpClient>> {
    let client = state.sftp.lock().unwrap().get(&id).cloned();
    Ok(client.ok_or(Error::SessionClosed)?)
}

#[tauri::command]
pub async fn sftp_close(state: State<'_, AppState>, sftp_id: Uuid) -> CommandResult<()> {
    let client = state.sftp.lock().unwrap().remove(&sftp_id);
    if let Some(client) = client {
        client.close().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    sftp_id: Uuid,
    path: String,
) -> CommandResult<Vec<FileEntry>> {
    Ok(client(&state, sftp_id)?.list(&path).await?)
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    sftp_id: Uuid,
    path: String,
) -> CommandResult<()> {
    Ok(client(&state, sftp_id)?.mkdir(&path).await?)
}

#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    sftp_id: Uuid,
    from: String,
    to: String,
) -> CommandResult<()> {
    Ok(client(&state, sftp_id)?.rename(&from, &to).await?)
}

/// Removes directories recursively.
#[tauri::command]
pub async fn sftp_remove(
    state: State<'_, AppState>,
    sftp_id: Uuid,
    path: String,
) -> CommandResult<()> {
    Ok(client(&state, sftp_id)?.remove(&path).await?)
}

#[tauri::command]
pub fn local_home(state: State<'_, AppState>) -> String {
    state
        .home_dir
        .as_ref()
        .map_or_else(|| "/".into(), |home| home.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn local_list(path: String) -> CommandResult<Vec<FileEntry>> {
    blocking(move || local_fs::list(Path::new(&path))).await
}

#[tauri::command]
pub async fn local_mkdir(path: String) -> CommandResult<()> {
    blocking(move || local_fs::mkdir(Path::new(&path))).await
}

#[tauri::command]
pub async fn local_rename(from: String, to: String) -> CommandResult<()> {
    blocking(move || local_fs::rename(Path::new(&from), Path::new(&to))).await
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferDirection {
    Upload,
    Download,
}

/// Copies `source` (file or directory) into `target_dir`. Resolves when done;
/// fails with kind `cancelled` after [`cancel_transfer`].
#[tauri::command]
pub async fn transfer(
    state: State<'_, AppState>,
    sftp_id: Uuid,
    transfer_id: Uuid,
    direction: TransferDirection,
    source: String,
    target_dir: String,
    on_progress: Channel<TransferProgress>,
) -> CommandResult<()> {
    let client = client(&state, sftp_id)?;
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .transfers
        .lock()
        .unwrap()
        .insert(transfer_id, cancel.clone());

    let report = |progress: &TransferProgress| {
        let _ = on_progress.send(progress.clone());
    };
    let result = match direction {
        TransferDirection::Upload => {
            client
                .upload(Path::new(&source), &target_dir, report, &cancel)
                .await
        }
        TransferDirection::Download => {
            client
                .download(&source, Path::new(&target_dir), report, &cancel)
                .await
        }
    };
    state.transfers.lock().unwrap().remove(&transfer_id);
    Ok(result?)
}

#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, transfer_id: Uuid) {
    if let Some(cancel) = state.transfers.lock().unwrap().get(&transfer_id) {
        cancel.store(true, Ordering::Relaxed);
    }
}
