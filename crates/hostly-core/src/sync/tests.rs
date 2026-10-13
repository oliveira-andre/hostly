//! Sync engine tests: several devices against an in-memory server that follows
//! the protocol's rules (newest change wins, increasing revisions).

use std::collections::BTreeMap;
use std::sync::Mutex;

use hostly_protocol::{Account, AuthMethod, Changes, Host, HostChange, HostRecord, PushResponse};
use uuid::Uuid;

use super::*;
use crate::hosts::HostStore;

#[derive(Default)]
struct FakeServer {
    state: Mutex<(BTreeMap<Uuid, HostRecord>, i64)>,
}

impl FakeServer {
    fn live_hosts(&self) -> Vec<Host> {
        let state = self.state.lock().unwrap();
        state.0.values().filter_map(|r| r.host.clone()).collect()
    }
}

impl SyncApi for FakeServer {
    async fn pull(&self, since: i64) -> Result<Changes> {
        let state = self.state.lock().unwrap();
        let mut records: Vec<_> = state
            .0
            .values()
            .filter(|r| r.revision > since)
            .cloned()
            .collect();
        records.sort_by_key(|r| r.revision);
        Ok(Changes {
            records,
            revision: state.1.max(since),
        })
    }

    async fn push(&self, changes: Vec<HostChange>) -> Result<PushResponse> {
        let mut state = self.state.lock().unwrap();
        let mut records = Vec::new();
        for change in changes {
            let newer_stored = state
                .0
                .get(&change.id)
                .is_some_and(|r| r.modified_at > change.modified_at);
            if !newer_stored {
                state.1 += 1;
                let record = HostRecord {
                    id: change.id,
                    host: change.host,
                    modified_at: change.modified_at,
                    revision: state.1,
                };
                state.0.insert(change.id, record);
            }
            records.push(state.0[&change.id].clone());
        }
        Ok(PushResponse { records })
    }
}

/// Runs `on_push` (e.g. a local edit) while a push is in flight.
struct EditDuringPush<'a, F: Fn() + Sync> {
    server: &'a FakeServer,
    on_push: F,
}

impl<F: Fn() + Sync> SyncApi for EditDuringPush<'_, F> {
    async fn pull(&self, since: i64) -> Result<Changes> {
        self.server.pull(since).await
    }

    async fn push(&self, changes: Vec<HostChange>) -> Result<PushResponse> {
        (self.on_push)();
        self.server.push(changes).await
    }
}

struct Device {
    _dir: tempfile::TempDir,
    hosts: Mutex<HostStore>,
    sync: Mutex<SyncFile>,
}

impl Device {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            hosts: Mutex::new(HostStore::open(dir.path().join("hosts.json")).unwrap()),
            sync: Mutex::new(SyncFile::open(dir.path().join("sync.json")).unwrap()),
            _dir: dir,
        }
    }

    /// Saves a host the way the app does: store it, then record the change.
    fn save(&self, host: Host, now: i64) -> Host {
        let host = self.hosts.lock().unwrap().upsert(host).unwrap();
        self.sync
            .lock()
            .unwrap()
            .record_local_change(host.id, false, now)
            .unwrap();
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
        self.sync
            .lock()
            .unwrap()
            .record_local_change(id, true, now)
            .unwrap();
    }

    fn log_in(&self) {
        let account = Account {
            id: Uuid::from_u128(1),
            email: "me@example.com".into(),
        };
        let login = Login::new("https://sync.test".into(), account);
        self.sync.lock().unwrap().set_login(Some(login)).unwrap();
    }

    fn log_out(&self) {
        self.sync.lock().unwrap().set_login(None).unwrap();
    }

    async fn sync(&self, api: &impl SyncApi, now: i64) -> SyncReport {
        sync_once(&self.hosts, &self.sync, api, now).await.unwrap()
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
    assert!(
        !desktop.sync(&server, 33).await.hosts_changed,
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
