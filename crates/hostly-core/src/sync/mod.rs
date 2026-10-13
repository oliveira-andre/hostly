//! Host sync with a Hostly server.
//!
//! - The first merge after logging in removes nothing. Hosts only on this device
//!   are uploaded, hosts only on the server are downloaded, and hosts are
//!   matched by id alone, so look-alike duplicates are kept on both sides.
//! - Afterwards, creates, edits and deletes flow both ways, and the change with
//!   the newest `modified_at` wins.
//! - Passwords and passphrases are never synced; they stay in each device's keychain.

mod api;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use hostly_protocol::{Account, Changes, HostChange, PushResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use api::ApiClient;

use crate::Result;
use crate::hosts::HostStore;
use crate::storage::{read_json, write_json};

/// Keychain entry holding the session token while logged in.
pub const SESSION_SECRET_KEY: &str = "sync/session";

/// Milliseconds since the Unix epoch, the unit of `modified_at`.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Login>,
    /// Local changes the server hasn't accepted yet. Recorded while logged out
    /// too, so edits made before logging in keep their timestamps.
    #[serde(default)]
    pub pending: BTreeMap<Uuid, PendingChange>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Login {
    pub server: String,
    pub account: Account,
    /// Revision of the last change pulled from the server.
    pub cursor: i64,
    /// Whether the first merge for this login has happened.
    pub merged: bool,
}

impl Login {
    pub fn new(server: String, account: Account) -> Self {
        Self {
            server,
            account,
            cursor: 0,
            merged: false,
        }
    }

    fn same_as(&self, other: &Login) -> bool {
        self.server == other.server && self.account.id == other.account.id
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingChange {
    pub modified_at: i64,
    pub deleted: bool,
}

/// Sync state, persisted as JSON next to the hosts file.
pub struct SyncFile {
    path: PathBuf,
    state: SyncState,
}

impl SyncFile {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let state = read_json(&path)?.unwrap_or_default();
        Ok(Self { path, state })
    }

    pub fn state(&self) -> &SyncState {
        &self.state
    }

    pub fn login(&self) -> Option<&Login> {
        self.state.login.as_ref()
    }

    pub fn set_login(&mut self, login: Option<Login>) -> Result<()> {
        let mut state = self.state.clone();
        state.login = login;
        self.replace(state)
    }

    /// Notes a host created, edited or deleted on this device.
    pub fn record_local_change(&mut self, id: Uuid, deleted: bool, now: i64) -> Result<()> {
        let mut state = self.state.clone();
        state.pending.insert(
            id,
            PendingChange {
                modified_at: now,
                deleted,
            },
        );
        self.replace(state)
    }

    fn replace(&mut self, state: SyncState) -> Result<()> {
        write_json(&self.path, &state)?;
        self.state = state;
        Ok(())
    }

    /// The state to update, if `login` is still the active login.
    fn state_for(&self, login: &Login) -> Option<SyncState> {
        self.login()
            .is_some_and(|current| current.same_as(login))
            .then(|| self.state.clone())
    }
}

/// What sync needs from a server; [`ApiClient`] is the real one.
pub trait SyncApi: Sync {
    fn pull(&self, since: i64) -> impl Future<Output = Result<Changes>> + Send;
    fn push(&self, changes: Vec<HostChange>) -> impl Future<Output = Result<PushResponse>> + Send;
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// The local host list changed, so the UI should reload it.
    pub hosts_changed: bool,
}

/// One sync round: the first merge if this login hasn't had one, then push
/// local changes, then pull remote ones. Locks are never held across network
/// calls, so hosts can be edited while a sync is in flight; if the login
/// changes meanwhile, the round's results are discarded.
///
/// Lock order: `hosts` before `sync`.
pub async fn sync_once(
    hosts: &Mutex<HostStore>,
    sync: &Mutex<SyncFile>,
    api: &impl SyncApi,
    now: i64,
) -> Result<SyncReport> {
    let Some(login) = sync.lock().unwrap().login().cloned() else {
        return Ok(SyncReport::default());
    };
    let mut hosts_changed = false;
    if !login.merged {
        hosts_changed |= merge(hosts, sync, api, &login, now).await?;
    }
    hosts_changed |= push(hosts, sync, api, &login).await?;
    hosts_changed |= pull(hosts, sync, api, &login).await?;
    Ok(SyncReport { hosts_changed })
}

/// The first merge after logging in.
async fn merge(
    hosts: &Mutex<HostStore>,
    sync: &Mutex<SyncFile>,
    api: &impl SyncApi,
    login: &Login,
    now: i64,
) -> Result<bool> {
    let remote = api.pull(0).await?;
    let mut store = hosts.lock().unwrap();
    let mut file = sync.lock().unwrap();
    let Some(mut state) = file.state_for(login) else {
        return Ok(false);
    };

    // Deletions made while logged out are dropped: the first merge removes nothing.
    state.pending.retain(|_, change| !change.deleted);
    let mut adopted = Vec::new();
    let mut on_server = HashSet::new();
    for record in remote.records {
        // A server tombstone never removes a local host; such hosts are uploaded again below.
        let Some(host) = record.host else { continue };
        on_server.insert(record.id);
        match state.pending.get(&record.id) {
            // Edited here more recently: this device's version is pushed next.
            Some(local) if local.modified_at > record.modified_at => {}
            _ => {
                state.pending.remove(&record.id);
                adopted.push(host);
            }
        }
    }
    for host in store.list() {
        if !on_server.contains(&host.id) {
            state.pending.entry(host.id).or_insert(PendingChange {
                modified_at: now,
                deleted: false,
            });
        }
    }

    let changed = store.apply_remote(adopted, &[])?;
    let merged = state.login.as_mut().expect("checked by state_for");
    merged.merged = true;
    merged.cursor = remote.revision;
    file.replace(state)?;
    Ok(changed)
}

async fn push(
    hosts: &Mutex<HostStore>,
    sync: &Mutex<SyncFile>,
    api: &impl SyncApi,
    login: &Login,
) -> Result<bool> {
    let changes: Vec<HostChange> = {
        let store = hosts.lock().unwrap();
        let mut file = sync.lock().unwrap();
        let Some(mut state) = file.state_for(login) else {
            return Ok(false);
        };
        // An edit whose host has since vanished without a recorded delete has nothing to send.
        let before = state.pending.len();
        state
            .pending
            .retain(|id, change| change.deleted || store.get(*id).is_some());
        let changes = state
            .pending
            .iter()
            .map(|(&id, change)| HostChange {
                id,
                host: (!change.deleted).then(|| store.get(id).cloned()).flatten(),
                modified_at: change.modified_at,
            })
            .collect();
        if state.pending.len() != before {
            file.replace(state)?;
        }
        changes
    };
    if changes.is_empty() {
        return Ok(false);
    }

    let sent: HashMap<Uuid, i64> = changes.iter().map(|c| (c.id, c.modified_at)).collect();
    let response = api.push(changes).await?;

    let mut store = hosts.lock().unwrap();
    let mut file = sync.lock().unwrap();
    let Some(mut state) = file.state_for(login) else {
        return Ok(false);
    };
    let (mut upserts, mut removals) = (Vec::new(), Vec::new());
    for record in response.records {
        let Some(&sent_at) = sent.get(&record.id) else {
            continue;
        };
        // Edited again while the push was in flight: that newer edit goes next round.
        if state.pending.get(&record.id).map(|c| c.modified_at) != Some(sent_at) {
            continue;
        }
        state.pending.remove(&record.id);
        // The server kept a newer change from another device: take it.
        if record.modified_at > sent_at {
            match record.host {
                Some(host) => upserts.push(host),
                None => removals.push(record.id),
            }
        }
    }
    let changed = store.apply_remote(upserts, &removals)?;
    file.replace(state)?;
    Ok(changed)
}

async fn pull(
    hosts: &Mutex<HostStore>,
    sync: &Mutex<SyncFile>,
    api: &impl SyncApi,
    login: &Login,
) -> Result<bool> {
    let cursor = {
        let file = sync.lock().unwrap();
        match file.login() {
            Some(current) if current.same_as(login) => current.cursor,
            _ => return Ok(false),
        }
    };
    let changes = api.pull(cursor).await?;

    let mut store = hosts.lock().unwrap();
    let mut file = sync.lock().unwrap();
    let Some(mut state) = file.state_for(login) else {
        return Ok(false);
    };
    let (mut upserts, mut removals) = (Vec::new(), Vec::new());
    for record in changes.records {
        if let Some(local) = state.pending.get(&record.id) {
            // This device's newer change is pushed next round.
            if local.modified_at > record.modified_at {
                continue;
            }
            state.pending.remove(&record.id);
        }
        match record.host {
            Some(host) => upserts.push(host),
            None => removals.push(record.id),
        }
    }
    let changed = store.apply_remote(upserts, &removals)?;
    state.login.as_mut().expect("checked by state_for").cursor = changes.revision;
    file.replace(state)?;
    Ok(changed)
}

#[cfg(test)]
mod tests;
