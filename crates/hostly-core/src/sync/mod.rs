//! Sync with a Hostly server: hosts, snippets, port-forwarding rules, and the
//! hosts' passwords and passphrases, sealed end to end (see [`crate::vault`]).
//!
//! - The first merge after logging in removes nothing. Items only on this
//!   device are uploaded, items only on the server are downloaded, and items
//!   are matched by kind and id alone, so look-alike duplicates are kept on
//!   both sides.
//! - Afterwards, creates, edits and deletes flow both ways, and the change with
//!   the newest `modified_at` wins.
//! - Secrets travel only sealed with the account's vault key. A device without
//!   that key leaves them alone until it has one; logging in again merges them.
//!   Received secrets wait in [`SyncState::inbox`] until the app writes them to
//!   the keychain, so a failed keychain write is retried rather than lost.

mod api;
pub mod secrets;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use hostly_protocol::{Account, Item, ItemChange, ItemChanges, ItemPushResponse, Kind};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use api::{ApiClient, LoggedIn};

use crate::Result;
use crate::hosts::HostStore;
use crate::storage::{read_json, write_json};
use crate::store::{ForwardStore, SnippetStore};

/// Keychain entry holding the session token while logged in.
pub const SESSION_SECRET_KEY: &str = "sync/session";

/// `sync.json` format. Version 1 syncs every kind through `/items`; a file
/// from before it merges again, so nothing synced under `/hosts` is missed.
const STATE_VERSION: u32 = 1;

/// Milliseconds since the Unix epoch, the unit of `modified_at`.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Identifies a synced item. Written as `kind/id` in `sync.json`; a bare id,
/// from before snippets and port forwards, is a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemKey {
    pub kind: Kind,
    pub id: Uuid,
}

impl ItemKey {
    pub fn new(kind: Kind, id: Uuid) -> Self {
        Self { kind, id }
    }
}

impl fmt::Display for ItemKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind.as_str(), self.id)
    }
}

impl FromStr for ItemKey {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, String> {
        let (kind, id) = match value.split_once('/') {
            Some((kind, id)) => (
                Kind::parse(kind).ok_or_else(|| format!("unknown kind {kind:?}"))?,
                id,
            ),
            None => (Kind::Host, value),
        };
        Ok(Self::new(kind, id.parse().map_err(|e| format!("{e}"))?))
    }
}

impl Serialize for ItemKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ItemKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncState {
    #[serde(default)]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Login>,
    /// Local changes the server hasn't accepted yet. Recorded while logged out
    /// too, so edits made before logging in keep their timestamps.
    #[serde(default)]
    pub pending: BTreeMap<ItemKey, PendingChange>,
    /// Secrets received from the server, by host id, not yet in the keychain:
    /// sealed, or `None` to delete.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inbox: BTreeMap<Uuid, Option<String>>,
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
        let mut file = Self {
            state: read_json(&path)?.unwrap_or_default(),
            path,
        };
        if file.state.version < STATE_VERSION {
            let mut state = file.state.clone();
            state.version = STATE_VERSION;
            if let Some(login) = &mut state.login {
                login.merged = false;
                login.cursor = 0;
            }
            file.replace(state)?;
        }
        Ok(file)
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

    /// Notes an item created, edited or deleted on this device. A secret
    /// changed here supersedes one still waiting in the inbox.
    pub fn record_local_change(&mut self, key: ItemKey, deleted: bool, now: i64) -> Result<()> {
        let mut state = self.state.clone();
        state.pending.insert(
            key,
            PendingChange {
                modified_at: now,
                deleted,
            },
        );
        if key.kind == Kind::Secret {
            state.inbox.remove(&key.id);
        }
        self.replace(state)
    }

    /// Removes a received secret once it is in the keychain, unless a newer
    /// one arrived meanwhile.
    pub fn take_from_inbox(&mut self, id: Uuid, written: &Option<String>) -> Result<()> {
        if self.state.inbox.get(&id) != Some(written) {
            return Ok(());
        }
        let mut state = self.state.clone();
        state.inbox.remove(&id);
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
    fn pull(&self, since: i64) -> impl Future<Output = Result<ItemChanges>> + Send;
    fn push(
        &self,
        changes: Vec<ItemChange>,
    ) -> impl Future<Output = Result<ItemPushResponse>> + Send;
}

/// This device's synced stores. Lock order: hosts, snippets, forwards, then
/// the sync file.
pub struct Local<'a> {
    pub hosts: &'a Mutex<HostStore>,
    pub snippets: &'a Mutex<SnippetStore>,
    pub forwards: &'a Mutex<ForwardStore>,
}

/// This device's secrets for one round, sealed with the vault key and keyed
/// by host id: `None` when the keychain holds none. A pending secret missing
/// here couldn't be read and waits for a later round.
pub type SealedSecrets = HashMap<Uuid, Option<String>>;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub hosts_changed: bool,
    pub snippets_changed: bool,
    pub forwards_changed: bool,
    /// Secrets arrived in the inbox.
    pub secrets_received: bool,
}

impl SyncReport {
    fn add(&mut self, other: SyncReport) {
        self.hosts_changed |= other.hosts_changed;
        self.snippets_changed |= other.snippets_changed;
        self.forwards_changed |= other.forwards_changed;
        self.secrets_received |= other.secrets_received;
    }
}

/// One sync round: the first merge if this login hasn't had one, then push
/// local changes, then pull remote ones. Locks are never held across network
/// calls, so items can be edited while a sync is in flight; if the login
/// changes meanwhile, the round's results are discarded.
///
/// `secrets` is `None` on a device without the vault key: secrets are then
/// neither sent nor applied.
pub async fn sync_once(
    local: &Local<'_>,
    sync: &Mutex<SyncFile>,
    api: &impl SyncApi,
    secrets: Option<&SealedSecrets>,
    now: i64,
) -> Result<SyncReport> {
    let Some(login) = sync.lock().unwrap().login().cloned() else {
        return Ok(SyncReport::default());
    };
    let round = Round {
        local,
        sync,
        api,
        secrets,
        login: &login,
    };
    let mut report = SyncReport::default();
    if !login.merged {
        report.add(round.merge(now).await?);
    }
    report.add(round.push().await?);
    report.add(round.pull().await?);
    Ok(report)
}

/// This device's version of an item.
enum Current {
    Present(serde_json::Value),
    Absent,
    /// A secret that couldn't be read this round.
    Unknown,
}

struct Stores<'a> {
    hosts: MutexGuard<'a, HostStore>,
    snippets: MutexGuard<'a, SnippetStore>,
    forwards: MutexGuard<'a, ForwardStore>,
}

impl Local<'_> {
    fn lock(&self) -> Stores<'_> {
        let hosts = self.hosts.lock().unwrap();
        let snippets = self.snippets.lock().unwrap();
        let forwards = self.forwards.lock().unwrap();
        Stores {
            hosts,
            snippets,
            forwards,
        }
    }
}

impl Stores<'_> {
    fn current(&self, key: ItemKey, secrets: Option<&SealedSecrets>) -> Current {
        let present =
            |item: Option<Item>| item.map_or(Current::Absent, |i| Current::Present(i.encode()));
        match key.kind {
            Kind::Host => present(self.hosts.get(key.id).cloned().map(Item::Host)),
            Kind::Snippet => present(self.snippets.get(key.id).cloned().map(Item::Snippet)),
            Kind::Forward => present(self.forwards.get(key.id).cloned().map(Item::Forward)),
            Kind::Secret => match secrets.and_then(|s| s.get(&key.id)) {
                Some(sealed) => present(sealed.clone().map(Item::Secret)),
                None => Current::Unknown,
            },
        }
    }

    /// Every item on this device, for the first merge.
    fn keys(&self, secrets: Option<&SealedSecrets>) -> Vec<ItemKey> {
        let hosts = self.hosts.list().iter().map(|h| (Kind::Host, h.id));
        let snippets = self.snippets.list().iter().map(|s| (Kind::Snippet, s.id));
        let forwards = self.forwards.list().iter().map(|f| (Kind::Forward, f.id));
        let sealed = secrets
            .into_iter()
            .flatten()
            .filter(|(_, sealed)| sealed.is_some())
            .map(|(&id, _)| (Kind::Secret, id));
        hosts
            .chain(snippets)
            .chain(forwards)
            .chain(sealed)
            .map(|(kind, id)| ItemKey::new(kind, id))
            .collect()
    }

    /// Applies remote changes: `None` deletes. Secrets go to the inbox.
    /// Items that don't decode or validate are skipped.
    fn apply(
        &mut self,
        changes: Vec<(ItemKey, Option<serde_json::Value>)>,
        inbox: &mut BTreeMap<Uuid, Option<String>>,
    ) -> Result<SyncReport> {
        let (mut hosts, mut snippets, mut forwards) = (Vec::new(), Vec::new(), Vec::new());
        let (mut host_removals, mut snippet_removals, mut forward_removals) =
            (Vec::new(), Vec::new(), Vec::new());
        let mut secrets_received = false;
        for (key, data) in changes {
            let Some(data) = data else {
                match key.kind {
                    Kind::Host => host_removals.push(key.id),
                    Kind::Snippet => snippet_removals.push(key.id),
                    Kind::Forward => forward_removals.push(key.id),
                    Kind::Secret => {
                        inbox.insert(key.id, None);
                        secrets_received = true;
                    }
                }
                continue;
            };
            match Item::decode(key.kind, key.id, data) {
                Ok(Item::Host(host)) => hosts.push(host),
                Ok(Item::Snippet(snippet)) => snippets.push(snippet),
                Ok(Item::Forward(forward)) => forwards.push(forward),
                Ok(Item::Secret(sealed)) => {
                    inbox.insert(key.id, Some(sealed));
                    secrets_received = true;
                }
                Err(_) => {}
            }
        }
        Ok(SyncReport {
            hosts_changed: self.hosts.apply_remote(hosts, &host_removals)?,
            snippets_changed: self.snippets.apply_remote(snippets, &snippet_removals)?,
            forwards_changed: self.forwards.apply_remote(forwards, &forward_removals)?,
            secrets_received,
        })
    }
}

struct Round<'a, A> {
    local: &'a Local<'a>,
    sync: &'a Mutex<SyncFile>,
    api: &'a A,
    secrets: Option<&'a SealedSecrets>,
    login: &'a Login,
}

impl<A: SyncApi> Round<'_, A> {
    /// Whether this round handles items of `kind`.
    fn syncs(&self, kind: Kind) -> bool {
        kind != Kind::Secret || self.secrets.is_some()
    }

    /// The first merge after logging in.
    async fn merge(&self, now: i64) -> Result<SyncReport> {
        let remote = self.api.pull(0).await?;
        let mut stores = self.local.lock();
        let mut file = self.sync.lock().unwrap();
        let Some(mut state) = file.state_for(self.login) else {
            return Ok(SyncReport::default());
        };

        // Deletions made while logged out are dropped: the first merge removes nothing.
        state.pending.retain(|_, change| !change.deleted);
        let mut adopted = Vec::new();
        let mut on_server = HashSet::new();
        for record in remote.records {
            let key = ItemKey::new(record.kind, record.id);
            // A server tombstone never removes a local item; such items are uploaded again below.
            let Some(data) = record.data.filter(|_| self.syncs(key.kind)) else {
                continue;
            };
            on_server.insert(key);
            match state.pending.get(&key) {
                // Edited here more recently: this device's version is pushed next.
                Some(local) if local.modified_at > record.modified_at => {}
                _ => {
                    state.pending.remove(&key);
                    adopted.push((key, Some(data)));
                }
            }
        }
        for key in stores.keys(self.secrets) {
            if !on_server.contains(&key) {
                state.pending.entry(key).or_insert(PendingChange {
                    modified_at: now,
                    deleted: false,
                });
            }
        }

        let report = stores.apply(adopted, &mut state.inbox)?;
        let merged = state.login.as_mut().expect("checked by state_for");
        merged.merged = true;
        merged.cursor = remote.revision;
        file.replace(state)?;
        Ok(report)
    }

    async fn push(&self) -> Result<SyncReport> {
        let changes: Vec<ItemChange> = {
            let stores = self.local.lock();
            let mut file = self.sync.lock().unwrap();
            let Some(mut state) = file.state_for(self.login) else {
                return Ok(SyncReport::default());
            };
            let before = state.pending.len();
            let mut changes = Vec::new();
            state.pending.retain(|&key, change| {
                if !self.syncs(key.kind) {
                    return true;
                }
                let data = if change.deleted {
                    None
                } else {
                    match stores.current(key, self.secrets) {
                        Current::Present(data) => Some(data),
                        // An edit whose item has since vanished without a recorded delete has nothing to send.
                        Current::Absent => return false,
                        Current::Unknown => return true,
                    }
                };
                changes.push(ItemChange {
                    kind: key.kind,
                    id: key.id,
                    data,
                    modified_at: change.modified_at,
                });
                true
            });
            if state.pending.len() != before {
                file.replace(state)?;
            }
            changes
        };
        if changes.is_empty() {
            return Ok(SyncReport::default());
        }

        let sent: HashMap<ItemKey, i64> = changes
            .iter()
            .map(|c| (ItemKey::new(c.kind, c.id), c.modified_at))
            .collect();
        let response = self.api.push(changes).await?;

        let mut stores = self.local.lock();
        let mut file = self.sync.lock().unwrap();
        let Some(mut state) = file.state_for(self.login) else {
            return Ok(SyncReport::default());
        };
        let mut adopted = Vec::new();
        for record in response.records {
            let key = ItemKey::new(record.kind, record.id);
            let Some(&sent_at) = sent.get(&key) else {
                continue;
            };
            // Edited again while the push was in flight: that newer edit goes next round.
            if state.pending.get(&key).map(|c| c.modified_at) != Some(sent_at) {
                continue;
            }
            state.pending.remove(&key);
            // The server kept a newer change from another device: take it.
            if record.modified_at > sent_at {
                adopted.push((key, record.data));
            }
        }
        let report = stores.apply(adopted, &mut state.inbox)?;
        file.replace(state)?;
        Ok(report)
    }

    async fn pull(&self) -> Result<SyncReport> {
        let cursor = {
            let file = self.sync.lock().unwrap();
            match file.login() {
                Some(current) if current.same_as(self.login) => current.cursor,
                _ => return Ok(SyncReport::default()),
            }
        };
        let changes = self.api.pull(cursor).await?;

        let mut stores = self.local.lock();
        let mut file = self.sync.lock().unwrap();
        let Some(mut state) = file.state_for(self.login) else {
            return Ok(SyncReport::default());
        };
        let mut adopted = Vec::new();
        for record in changes.records {
            let key = ItemKey::new(record.kind, record.id);
            if !self.syncs(key.kind) {
                continue;
            }
            if let Some(local) = state.pending.get(&key) {
                // This device's newer change is pushed next round.
                if local.modified_at > record.modified_at {
                    continue;
                }
                state.pending.remove(&key);
            }
            adopted.push((key, record.data));
        }
        let report = stores.apply(adopted, &mut state.inbox)?;
        state.login.as_mut().expect("checked by state_for").cursor = changes.revision;
        file.replace(state)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests;
