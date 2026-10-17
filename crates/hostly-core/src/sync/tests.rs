//! Sync engine tests: several devices against an in-memory server that follows
//! the protocol's rules (newest change wins, increasing revisions).

use std::collections::BTreeMap;
use std::sync::Mutex;

use hostly_protocol::{
    Account, AuthMethod, Forward, ForwardKind, Host, ItemChange, ItemChanges, ItemPushResponse,
    ItemRecord, Kind, Snippet,
};
use uuid::Uuid;

use super::*;
use crate::hosts::HostStore;
use crate::secrets::{MemoryStore, SecretStore, host_secret_key};
use crate::store::{ForwardStore, SnippetStore};
use crate::vault::VaultKey;

#[derive(Default)]
struct FakeServer {
    state: Mutex<(BTreeMap<ItemKey, ItemRecord>, i64)>,
}

impl FakeServer {
    fn live(&self, kind: Kind) -> Vec<serde_json::Value> {
        let state = self.state.lock().unwrap();
        state
            .0
            .values()
            .filter(|r| r.kind == kind)
            .filter_map(|r| r.data.clone())
            .collect()
    }

    fn live_hosts(&self) -> Vec<Host> {
        self.live(Kind::Host)
            .into_iter()
            .map(|data| serde_json::from_value(data).unwrap())
            .collect()
    }
}

impl SyncApi for FakeServer {
    async fn pull(&self, since: i64) -> Result<ItemChanges> {
        let state = self.state.lock().unwrap();
        let mut records: Vec<_> = state
            .0
            .values()
            .filter(|r| r.revision > since)
            .cloned()
            .collect();
        records.sort_by_key(|r| r.revision);
        Ok(ItemChanges {
            records,
            revision: state.1.max(since),
        })
    }

    async fn push(&self, changes: Vec<ItemChange>) -> Result<ItemPushResponse> {
        let mut state = self.state.lock().unwrap();
        let mut records = Vec::new();
        for change in changes {
            let key = ItemKey::new(change.kind, change.id);
            let newer_stored = state
                .0
                .get(&key)
                .is_some_and(|r| r.modified_at > change.modified_at);
            if !newer_stored {
                state.1 += 1;
                let record = ItemRecord {
                    kind: change.kind,
                    id: change.id,
                    data: change.data,
                    modified_at: change.modified_at,
                    revision: state.1,
                };
                state.0.insert(key, record);
            }
            records.push(state.0[&key].clone());
        }
        Ok(ItemPushResponse { records })
    }
}

/// Runs `on_push` (e.g. a local edit) while a push is in flight.
struct EditDuringPush<'a, F: Fn() + Sync> {
    server: &'a FakeServer,
    on_push: F,
}

impl<F: Fn() + Sync> SyncApi for EditDuringPush<'_, F> {
    async fn pull(&self, since: i64) -> Result<ItemChanges> {
        self.server.pull(since).await
    }

    async fn push(&self, changes: Vec<ItemChange>) -> Result<ItemPushResponse> {
        (self.on_push)();
        self.server.push(changes).await
    }
}

/// One device: its stores, keychain and sync file, used the way the app does.
struct Device {
    _dir: tempfile::TempDir,
    hosts: Mutex<HostStore>,
    snippets: Mutex<SnippetStore>,
    forwards: Mutex<ForwardStore>,
    sync: Mutex<SyncFile>,
    keychain: MemoryStore,
    /// The account's vault key, once this device has it.
    vault: Mutex<Option<VaultKey>>,
}

impl Device {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        Self {
            hosts: Mutex::new(HostStore::open(path("hosts.json")).unwrap()),
            snippets: Mutex::new(SnippetStore::open(path("snippets.json")).unwrap()),
            forwards: Mutex::new(ForwardStore::open(path("forwards.json")).unwrap()),
            sync: Mutex::new(SyncFile::open(path("sync.json")).unwrap()),
            keychain: MemoryStore::default(),
            vault: Mutex::new(None),
            _dir: dir,
        }
    }

    fn record(&self, kind: Kind, id: Uuid, deleted: bool, now: i64) {
        self.sync
            .lock()
            .unwrap()
            .record_local_change(ItemKey::new(kind, id), deleted, now)
            .unwrap();
    }

    /// Saves a host the way the app does: store it, then record the change.
    fn save(&self, host: Host, now: i64) -> Host {
        let host = self.hosts.lock().unwrap().upsert(host).unwrap();
        self.record(Kind::Host, host.id, false, now);
        host
    }

    fn add(&self, label: &str, hostname: &str, now: i64) -> Host {
        self.save(host(label, hostname), now)
    }

    fn rename(&self, id: Uuid, label: &str, now: i64) {
        let mut host = self.hosts.lock().unwrap().get(id).unwrap().clone();
        host.label = label.into();
        self.save(host, now);
    }

    fn delete(&self, id: Uuid, now: i64) {
        self.hosts.lock().unwrap().remove(id).unwrap();
        self.record(Kind::Host, id, true, now);
        self.set_secret(id, None, now);
    }

    fn save_snippet(&self, snippet: Snippet, now: i64) -> Snippet {
        let snippet = self.snippets.lock().unwrap().upsert(snippet).unwrap();
        self.record(Kind::Snippet, snippet.id, false, now);
        snippet
    }

    fn save_forward(&self, forward: Forward, now: i64) -> Forward {
        let forward = self.forwards.lock().unwrap().upsert(forward).unwrap();
        self.record(Kind::Forward, forward.id, false, now);
        forward
    }

    /// Saves (or with `None` deletes) a host's password in the keychain.
    fn set_secret(&self, id: Uuid, secret: Option<&str>, now: i64) {
        let key = host_secret_key(id);
        match secret {
            Some(secret) => self.keychain.set(&key, secret).unwrap(),
            None => self.keychain.delete(&key).unwrap(),
        }
        self.record(Kind::Secret, id, secret.is_none(), now);
    }

    fn secret(&self, id: Uuid) -> Option<String> {
        self.keychain.get(&host_secret_key(id)).unwrap()
    }

    /// Logs in; with `vault`, as an end-to-end encrypted account.
    fn log_in_with(&self, vault: Option<&VaultKey>) {
        let account = Account {
            id: Uuid::from_u128(1),
            email: "me@example.com".into(),
        };
        let login = Login::new("https://sync.test".into(), account);
        self.sync.lock().unwrap().set_login(Some(login)).unwrap();
        *self.vault.lock().unwrap() = vault.cloned();
    }

    fn log_in(&self) {
        self.log_in_with(None);
    }

    fn log_out(&self) {
        self.sync.lock().unwrap().set_login(None).unwrap();
        *self.vault.lock().unwrap() = None;
    }

    /// One round the way the app runs it: seal the secrets it needs, sync,
    /// then write received secrets to the keychain.
    async fn sync(&self, api: &impl SyncApi, now: i64) -> SyncReport {
        let vault = self.vault.lock().unwrap().clone();
        let sealed = vault.as_ref().map(|vault| {
            let ids = secrets::wanted(&self.hosts, &self.sync);
            secrets::seal(&ids, &self.keychain, vault)
        });
        let local = Local {
            hosts: &self.hosts,
            snippets: &self.snippets,
            forwards: &self.forwards,
        };
        let report = sync_once(&local, &self.sync, api, sealed.as_ref(), now)
            .await
            .unwrap();
        if let Some(vault) = &vault {
            secrets::deliver(&self.sync, &self.keychain, vault).unwrap();
        }
        report
    }

    /// (label, hostname) pairs, sorted, duplicates included.
    fn summary(&self) -> Vec<(String, String)> {
        summarize(self.hosts.lock().unwrap().list())
    }

    fn get(&self, id: Uuid) -> Option<Host> {
        self.hosts.lock().unwrap().get(id).cloned()
    }

    fn pending(&self) -> usize {
        self.sync.lock().unwrap().state().pending.len()
    }
}

fn host(label: &str, hostname: &str) -> Host {
    Host {
        id: Uuid::new_v4(),
        label: label.into(),
        hostname: hostname.into(),
        port: 22,
        username: "deploy".into(),
        auth: AuthMethod::Password,
        group: None,
        mosh: false,
    }
}

fn snippet(name: &str, script: &str) -> Snippet {
    Snippet {
        id: Uuid::new_v4(),
        name: name.into(),
        script: script.into(),
    }
}

fn summarize(hosts: &[Host]) -> Vec<(String, String)> {
    let mut summary: Vec<_> = hosts
        .iter()
        .map(|h| (h.label.clone(), h.hostname.clone()))
        .collect();
    summary.sort();
    summary
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<_> = items
        .iter()
        .map(|(l, h)| (l.to_string(), h.to_string()))
        .collect();
    pairs.sort();
    pairs
}

#[tokio::test]
async fn first_merge_keeps_every_host_including_look_alike_duplicates() {
    let server = FakeServer::default();
    let laptop = Device::new();
    laptop.add("storage", "10.0.0.1", 1);
    laptop.add("apps", "10.0.0.2", 2);
    laptop.log_in();
    laptop.sync(&server, 10).await;

    // The desktop already has its own "storage" (same address, different id) and a local-only host.
    let desktop = Device::new();
    desktop.add("storage", "10.0.0.1", 3);
    desktop.add("db", "10.0.0.3", 4);
    desktop.log_in();
    let report = desktop.sync(&server, 20).await;
    assert!(report.hosts_changed);

    let everything = pairs(&[
        ("apps", "10.0.0.2"),
        ("db", "10.0.0.3"),
        ("storage", "10.0.0.1"),
        ("storage", "10.0.0.1"),
    ]);
    assert_eq!(desktop.summary(), everything);
    laptop.sync(&server, 30).await;
    assert_eq!(laptop.summary(), everything);
    assert_eq!(summarize(&server.live_hosts()), everything);
    assert_eq!((laptop.pending(), desktop.pending()), (0, 0));
}

#[tokio::test]
async fn first_merge_never_removes_a_local_host_even_if_the_server_deleted_it() {
    let server = FakeServer::default();
    let laptop = Device::new();
    let shared = laptop.add("shared", "10.0.0.1", 1);
    laptop.log_in();
    laptop.sync(&server, 2).await;
    laptop.log_out();

    // Deleted elsewhere while the laptop was logged out.
    let desktop = Device::new();
    desktop.log_in();
    desktop.sync(&server, 3).await;
    desktop.delete(shared.id, 4);
    desktop.sync(&server, 5).await;
    assert!(server.live_hosts().is_empty());

    laptop.log_in();
    laptop.sync(&server, 6).await;
    assert!(
        laptop.get(shared.id).is_some(),
        "the merge must not delete local hosts"
    );
    desktop.sync(&server, 7).await;
    assert!(
        desktop.get(shared.id).is_some(),
        "the kept host is uploaded again"
    );
}

#[tokio::test]
async fn creates_edits_and_deletes_flow_both_ways_after_the_merge() {
    let server = FakeServer::default();
    let (laptop, desktop) = (Device::new(), Device::new());
    laptop.log_in();
    desktop.log_in();
    laptop.sync(&server, 1).await;
    desktop.sync(&server, 1).await;

    let web = laptop.add("web", "10.0.0.5", 10);
    laptop.sync(&server, 11).await;
    assert!(desktop.sync(&server, 12).await.hosts_changed);
    assert_eq!(desktop.get(web.id), Some(web.clone()));

    desktop.rename(web.id, "web-prod", 20);
    desktop.sync(&server, 21).await;
    laptop.sync(&server, 22).await;
    assert_eq!(laptop.get(web.id).unwrap().label, "web-prod");

    laptop.delete(web.id, 30);
    laptop.sync(&server, 31).await;
    desktop.sync(&server, 32).await;
    assert_eq!(desktop.get(web.id), None);
    assert_eq!(
        desktop.sync(&server, 33).await,
        SyncReport::default(),
        "a quiet round changes nothing"
    );
}

#[tokio::test]
async fn the_newest_edit_wins_whichever_device_syncs_first() {
    let server = FakeServer::default();
    let (laptop, desktop) = (Device::new(), Device::new());
    let shared = laptop.add("shared", "10.0.0.1", 1);
    laptop.log_in();
    laptop.sync(&server, 2).await;
    desktop.log_in();
    desktop.sync(&server, 3).await;

    // The laptop's edit is older but reaches the server first.
    laptop.rename(shared.id, "older", 10);
    desktop.rename(shared.id, "newer", 20);
    laptop.sync(&server, 21).await;
    desktop.sync(&server, 22).await;
    laptop.sync(&server, 23).await;
    assert_eq!(laptop.get(shared.id).unwrap().label, "newer");
    assert_eq!(desktop.get(shared.id).unwrap().label, "newer");

    // The newer edit reaches the server first; the older push is refused and corrected.
    desktop.rename(shared.id, "newest", 40);
    laptop.rename(shared.id, "stale", 30);
    desktop.sync(&server, 41).await;
    assert!(laptop.sync(&server, 42).await.hosts_changed);
    assert_eq!(laptop.get(shared.id).unwrap().label, "newest");
}

#[tokio::test]
async fn edits_made_while_logged_out_win_when_newer() {
    let server = FakeServer::default();
    let laptop = Device::new();
    let shared = laptop.add("shared", "10.0.0.1", 1);
    laptop.log_in();
    laptop.sync(&server, 2).await;
    laptop.log_out();

    laptop.rename(shared.id, "edited offline", 50);
    laptop.log_in();
    laptop.sync(&server, 51).await;
    assert_eq!(server.live_hosts()[0].label, "edited offline");
}

#[tokio::test]
async fn deletions_made_while_logged_out_are_not_applied_by_the_merge() {
    let server = FakeServer::default();
    let laptop = Device::new();
    let shared = laptop.add("shared", "10.0.0.1", 1);
    laptop.log_in();
    laptop.sync(&server, 2).await;
    laptop.log_out();

    laptop.delete(shared.id, 3);
    laptop.log_in();
    laptop.sync(&server, 4).await;
    assert_eq!(
        laptop.get(shared.id),
        Some(shared),
        "restored from the account"
    );
    assert_eq!(server.live_hosts().len(), 1);
}

#[tokio::test]
async fn logging_in_again_does_not_duplicate_hosts() {
    let server = FakeServer::default();
    let laptop = Device::new();
    laptop.add("storage", "10.0.0.1", 1);
    laptop.add("apps", "10.0.0.2", 2);
    for round in 0..3 {
        laptop.log_in();
        laptop.sync(&server, 10 + round).await;
        laptop.log_out();
    }
    assert_eq!(laptop.summary().len(), 2);
    assert_eq!(server.live_hosts().len(), 2);
}

#[tokio::test]
async fn an_edit_made_during_a_push_is_kept_and_sent_next_round() {
    let server = FakeServer::default();
    let laptop = Device::new();
    let shared = laptop.add("shared", "10.0.0.1", 1);
    laptop.log_in();
    laptop.sync(&server, 2).await;

    laptop.rename(shared.id, "first", 10);
    let racing = EditDuringPush {
        server: &server,
        on_push: || laptop.rename(shared.id, "second", 11),
    };
    laptop.sync(&racing, 12).await;
    assert_eq!(laptop.get(shared.id).unwrap().label, "second");
    assert_eq!(
        laptop.pending(),
        1,
        "the in-flight edit still needs pushing"
    );

    laptop.sync(&server, 13).await;
    assert_eq!(server.live_hosts()[0].label, "second");
    assert_eq!(laptop.pending(), 0);
}

#[tokio::test]
async fn nothing_happens_while_logged_out() {
    let server = FakeServer::default();
    let laptop = Device::new();
    laptop.add("storage", "10.0.0.1", 1);
    assert_eq!(laptop.sync(&server, 2).await, SyncReport::default());
    assert!(server.live_hosts().is_empty());
    assert_eq!(laptop.pending(), 1, "kept for when the device logs in");
}

#[tokio::test]
async fn snippets_and_forwards_sync_like_hosts() {
    let server = FakeServer::default();
    let (laptop, desktop) = (Device::new(), Device::new());
    let db = laptop.add("db", "10.0.0.9", 1);
    let uptime = laptop.save_snippet(snippet("uptime", "uptime"), 2);
    let tunnel = laptop.save_forward(
        Forward {
            id: Uuid::new_v4(),
            label: "postgres".into(),
            host_id: db.id,
            kind: ForwardKind::Local,
            bind_address: "127.0.0.1".into(),
            bind_port: 5433,
            destination_host: Some("localhost".into()),
            destination_port: Some(5432),
        },
        3,
    );
    laptop.log_in();
    laptop.sync(&server, 10).await;

    desktop.log_in();
    let report = desktop.sync(&server, 11).await;
    assert!(report.hosts_changed && report.snippets_changed && report.forwards_changed);
    assert_eq!(
        desktop.snippets.lock().unwrap().list(),
        std::slice::from_ref(&uptime)
    );
    assert_eq!(
        desktop.forwards.lock().unwrap().list(),
        std::slice::from_ref(&tunnel)
    );

    let mut edited = uptime.clone();
    edited.script = "uptime\nwho".into();
    desktop.save_snippet(edited.clone(), 20);
    desktop.forwards.lock().unwrap().remove(tunnel.id).unwrap();
    desktop.record(Kind::Forward, tunnel.id, true, 21);
    desktop.sync(&server, 22).await;

    let report = laptop.sync(&server, 23).await;
    assert!(report.snippets_changed && report.forwards_changed && !report.hosts_changed);
    assert_eq!(laptop.snippets.lock().unwrap().list(), &[edited]);
    assert!(laptop.forwards.lock().unwrap().list().is_empty());
}

#[tokio::test]
async fn secrets_travel_sealed_and_land_in_the_other_devices_keychain() {
    let server = FakeServer::default();
    let vault = VaultKey::generate().unwrap();
    let (laptop, desktop) = (Device::new(), Device::new());
    let db = laptop.add("db", "10.0.0.9", 1);
    laptop.set_secret(db.id, Some("hunter2"), 2);
    laptop.log_in_with(Some(&vault));
    laptop.sync(&server, 10).await;

    let stored = server.live(Kind::Secret);
    assert_eq!(stored.len(), 1);
    assert!(
        !stored[0].to_string().contains("hunter2"),
        "never in the clear"
    );

    desktop.log_in_with(Some(&vault));
    assert!(desktop.sync(&server, 11).await.secrets_received);
    assert_eq!(desktop.secret(db.id).as_deref(), Some("hunter2"));
    assert!(desktop.sync.lock().unwrap().state().inbox.is_empty());

    // A changed password flows back; deleting the host deletes it everywhere.
    desktop.set_secret(db.id, Some("correct horse"), 20);
    desktop.sync(&server, 21).await;
    laptop.sync(&server, 22).await;
    assert_eq!(laptop.secret(db.id).as_deref(), Some("correct horse"));
    laptop.delete(db.id, 30);
    laptop.sync(&server, 31).await;
    desktop.sync(&server, 32).await;
    assert_eq!(desktop.get(db.id), None);
    assert_eq!(desktop.secret(db.id), None);
}

#[tokio::test]
async fn the_first_merge_uploads_existing_keychain_secrets_and_keeps_the_servers() {
    let server = FakeServer::default();
    let vault = VaultKey::generate().unwrap();
    let laptop = Device::new();
    let db = laptop.add("db", "10.0.0.9", 1);
    let web = laptop.add("web", "10.0.0.10", 2);
    laptop.set_secret(db.id, Some("from laptop"), 3);
    laptop.log_in_with(Some(&vault));
    laptop.sync(&server, 10).await;

    // The desktop saved its own password for `db` before ever syncing, and
    // one for `web`, which the server doesn't have yet.
    let desktop = Device::new();
    desktop
        .hosts
        .lock()
        .unwrap()
        .apply_remote(vec![db.clone(), web.clone()], &[])
        .unwrap();
    desktop
        .keychain
        .set(&host_secret_key(db.id), "from desktop")
        .unwrap();
    desktop
        .keychain
        .set(&host_secret_key(web.id), "web pass")
        .unwrap();
    desktop.log_in_with(Some(&vault));
    desktop.sync(&server, 20).await;
    assert_eq!(desktop.secret(db.id).as_deref(), Some("from laptop"));

    laptop.sync(&server, 21).await;
    assert_eq!(laptop.secret(web.id).as_deref(), Some("web pass"));
}

#[tokio::test]
async fn a_device_without_the_vault_key_leaves_secrets_alone_until_it_has_it() {
    let server = FakeServer::default();
    let vault = VaultKey::generate().unwrap();
    let laptop = Device::new();
    let db = laptop.add("db", "10.0.0.9", 1);
    laptop.set_secret(db.id, Some("hunter2"), 2);
    laptop.log_in_with(Some(&vault));
    laptop.sync(&server, 3).await;

    // Logged in before the account had a vault key: hosts sync, secrets don't.
    let desktop = Device::new();
    desktop.log_in();
    let other = desktop.add("other", "10.0.0.11", 4);
    desktop.set_secret(other.id, Some("local only"), 5);
    let report = desktop.sync(&server, 6).await;
    assert!(report.hosts_changed && !report.secrets_received);
    assert_eq!(desktop.secret(db.id), None);
    assert_eq!(server.live(Kind::Secret).len(), 1);
    assert_eq!(desktop.pending(), 1, "the secret waits for the vault key");

    // Logging in again with the key merges secrets both ways.
    desktop.log_in_with(Some(&vault));
    desktop.sync(&server, 7).await;
    assert_eq!(desktop.secret(db.id).as_deref(), Some("hunter2"));
    laptop.sync(&server, 8).await;
    assert_eq!(laptop.secret(other.id).as_deref(), Some("local only"));
}

#[tokio::test]
async fn a_secret_changed_here_supersedes_one_waiting_in_the_inbox() {
    let dir = tempfile::tempdir().unwrap();
    let mut file = SyncFile::open(dir.path().join("sync.json")).unwrap();
    let id = Uuid::new_v4();
    let mut state = file.state().clone();
    state.inbox.insert(id, Some("v1.old".into()));
    file.replace(state).unwrap();

    file.record_local_change(ItemKey::new(Kind::Secret, id), false, 1)
        .unwrap();
    assert!(file.state().inbox.is_empty());
    // Taking an entry that changed meanwhile leaves the newer one.
    let mut state = file.state().clone();
    state.inbox.insert(id, Some("v1.newer".into()));
    file.replace(state).unwrap();
    file.take_from_inbox(id, &Some("v1.old".into())).unwrap();
    assert_eq!(file.state().inbox.len(), 1);
}

#[test]
fn a_sync_file_from_before_items_merges_again_and_keeps_its_pending_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sync.json");
    let id = Uuid::new_v4();
    let old = serde_json::json!({
        "login": {
            "server": "https://sync.test",
            "account": { "id": Uuid::from_u128(1), "email": "me@example.com" },
            "cursor": 42,
            "merged": true
        },
        "pending": { id.to_string(): { "modifiedAt": 5, "deleted": false } }
    });
    std::fs::write(&path, old.to_string()).unwrap();

    let file = SyncFile::open(&path).unwrap();
    let login = file.login().unwrap();
    assert_eq!((login.cursor, login.merged), (0, false));
    assert_eq!(file.state().version, STATE_VERSION);
    let key = ItemKey::new(Kind::Host, id);
    assert_eq!(file.state().pending[&key].modified_at, 5);

    // Rewritten in the new format, which reads back the same.
    let reopened = SyncFile::open(&path).unwrap();
    assert_eq!(reopened.state(), file.state());
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains(&format!("host/{id}"))
    );
}

#[test]
fn item_keys_print_and_parse() {
    let id = Uuid::new_v4();
    let key = ItemKey::new(Kind::Forward, id);
    assert_eq!(key.to_string(), format!("forward/{id}"));
    assert_eq!(key.to_string().parse::<ItemKey>().unwrap(), key);
    assert_eq!(
        id.to_string().parse::<ItemKey>().unwrap(),
        ItemKey::new(Kind::Host, id)
    );
    assert!(format!("gadget/{id}").parse::<ItemKey>().is_err());
}
