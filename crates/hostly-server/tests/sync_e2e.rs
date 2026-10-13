//! End-to-end sync: devices running the app's real sync engine and HTTP client
//! (`hostly_core::sync`) against this server and a real PostgreSQL database.
//!
//! Needs `TEST_DATABASE_URL` (or `.env`); without it the tests pass after a skip notice.

use std::sync::Mutex;

use hostly_core::Error;
use hostly_core::hosts::HostStore;
use hostly_core::sync::{ApiClient, Login, SyncFile, SyncReport, sync_once};
use hostly_protocol::{AuthMethod, Credentials, Host, Session, error_code};
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery";

/// Serves the app on a random port and returns its base URL, or returns from
/// the test when no database is configured.
macro_rules! server {
    () => {
        match start_server().await {
            Some(base) => base,
            None => return,
        }
    };
}

async fn start_server() -> Option<String> {
    let _ = dotenvy::dotenv();
    let Some(url) = std::env::var("TEST_DATABASE_URL")
        .ok()
        .filter(|url| !url.is_empty())
    else {
        eprintln!("skipping: TEST_DATABASE_URL is not set");
        return None;
    };
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connect to TEST_DATABASE_URL");
    hostly_server::migrate(&pool).await.unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, hostly_server::app(pool))
            .await
            .unwrap()
    });
    Some(base)
}

/// A fresh account; each test gets its own so tests can share the database.
async fn sign_up(base: &str) -> Credentials {
    let credentials = Credentials {
        email: format!("sync-{}@example.com", Uuid::new_v4()),
        password: PASSWORD.into(),
    };
    ApiClient::new(base, None)
        .unwrap()
        .sign_up(&credentials)
        .await
        .unwrap();
    credentials
}

struct Device {
    _dir: tempfile::TempDir,
    hosts: Mutex<HostStore>,
    sync: Mutex<SyncFile>,
    api: Option<ApiClient>,
}

impl Device {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            hosts: Mutex::new(HostStore::open(dir.path().join("hosts.json")).unwrap()),
            sync: Mutex::new(SyncFile::open(dir.path().join("sync.json")).unwrap()),
            api: None,
            _dir: dir,
        }
    }

    /// Saves a host the way the app does: store it, then record the change.
    fn add(&self, label: &str, hostname: &str, now: i64) -> Host {
        let host = Host {
            id: Uuid::new_v4(),
            label: label.into(),
            hostname: hostname.into(),
            port: 22,
            username: "deploy".into(),
            auth: AuthMethod::Password,
            group: None,
        };
        self.save(host, now)
    }

    fn save(&self, host: Host, now: i64) -> Host {
        let host = self.hosts.lock().unwrap().upsert(host).unwrap();
        self.sync
            .lock()
            .unwrap()
            .record_local_change(host.id, false, now)
            .unwrap();
        host
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

    /// Logs in through the real API, as the account page does.
    async fn log_in(&mut self, base: &str, credentials: &Credentials) -> Session {
        let session = ApiClient::new(base, None)
            .unwrap()
            .log_in(credentials)
            .await
            .unwrap();
        let login = Login::new(base.into(), session.account.clone());
        self.sync.lock().unwrap().set_login(Some(login)).unwrap();
        self.api = Some(ApiClient::new(base, Some(session.token.clone())).unwrap());
        session
    }

    async fn log_out(&mut self) {
        if let Some(api) = self.api.take() {
            api.log_out().await.unwrap();
        }
        self.sync.lock().unwrap().set_login(None).unwrap();
    }

    async fn try_sync(&self, now: i64) -> hostly_core::Result<SyncReport> {
        sync_once(&self.hosts, &self.sync, self.api.as_ref().unwrap(), now).await
    }

    async fn sync(&self, now: i64) -> SyncReport {
        self.try_sync(now).await.unwrap()
    }

    /// (label, hostname) pairs, sorted, duplicates included.
    fn summary(&self) -> Vec<(String, String)> {
        let mut summary: Vec<_> = self
            .hosts
            .lock()
            .unwrap()
            .list()
            .iter()
            .map(|h| (h.label.clone(), h.hostname.clone()))
            .collect();
        summary.sort();
        summary
    }

    fn get(&self, id: Uuid) -> Option<Host> {
        self.hosts.lock().unwrap().get(id).cloned()
    }
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
async fn two_devices_merge_without_losing_anything_then_stay_in_sync() {
    let base = server!();
    let credentials = sign_up(&base).await;

    let mut laptop = Device::new();
    laptop.add("storage", "10.0.0.1", 1);
    let apps = laptop.add("apps", "10.0.0.2", 2);
    laptop.log_in(&base, &credentials).await;
    laptop.sync(10).await;

    // The desktop has its own look-alike "storage" and a host only it knows.
    let mut desktop = Device::new();
    desktop.add("storage", "10.0.0.1", 3);
    let db = desktop.add("db", "10.0.0.3", 4);
    desktop.log_in(&base, &credentials).await;
    assert!(desktop.sync(20).await.hosts_changed);
    laptop.sync(21).await;

    let merged = pairs(&[
        ("apps", "10.0.0.2"),
        ("db", "10.0.0.3"),
        ("storage", "10.0.0.1"),
        ("storage", "10.0.0.1"),
    ]);
    assert_eq!(laptop.summary(), merged);
    assert_eq!(desktop.summary(), merged);

    // Afterwards creates, edits and deletes flow both ways.
    let web = laptop.add("web", "10.0.0.4", 30);
    desktop.rename(db.id, "db-primary", 31);
    laptop.delete(apps.id, 32);
    laptop.sync(33).await;
    desktop.sync(34).await;
    laptop.sync(35).await;

    let after = pairs(&[
        ("db-primary", "10.0.0.3"),
        ("storage", "10.0.0.1"),
        ("storage", "10.0.0.1"),
        ("web", "10.0.0.4"),
    ]);
    assert_eq!(laptop.summary(), after);
    assert_eq!(desktop.summary(), after);
    assert_eq!(desktop.get(web.id), laptop.get(web.id));
    assert!(
        !desktop.sync(36).await.hosts_changed,
        "nothing left to sync"
    );
}

#[tokio::test]
async fn the_newest_edit_wins_on_the_real_server() {
    let base = server!();
    let credentials = sign_up(&base).await;
    let (mut laptop, mut desktop) = (Device::new(), Device::new());
    let shared = laptop.add("shared", "10.0.0.1", 1);
    laptop.log_in(&base, &credentials).await;
    laptop.sync(2).await;
    desktop.log_in(&base, &credentials).await;
    desktop.sync(3).await;

    // The newer edit reaches the server first; the older one is refused and corrected.
    desktop.rename(shared.id, "newer", 20);
    laptop.rename(shared.id, "older", 10);
    desktop.sync(21).await;
    assert!(laptop.sync(22).await.hosts_changed);
    assert_eq!(laptop.get(shared.id).unwrap().label, "newer");
}

#[tokio::test]
async fn logging_out_keeps_hosts_and_logging_back_in_does_not_duplicate() {
    let base = server!();
    let credentials = sign_up(&base).await;
    let mut laptop = Device::new();
    laptop.add("storage", "10.0.0.1", 1);
    laptop.add("apps", "10.0.0.2", 2);

    for round in 0..3 {
        laptop.log_in(&base, &credentials).await;
        laptop.sync(10 + round).await;
        laptop.log_out().await;
        assert_eq!(laptop.summary().len(), 2, "logging out keeps the hosts");
    }

    let mut desktop = Device::new();
    desktop.log_in(&base, &credentials).await;
    desktop.sync(20).await;
    assert_eq!(
        desktop.summary().len(),
        2,
        "the account holds no duplicates"
    );
}

#[tokio::test]
async fn a_revoked_session_is_reported_as_unauthorized() {
    let base = server!();
    let credentials = sign_up(&base).await;
    let mut laptop = Device::new();
    let session = laptop.log_in(&base, &credentials).await;
    laptop.sync(1).await;

    // Log out with the same token elsewhere, e.g. from another copy of this session.
    ApiClient::new(&base, Some(session.token))
        .unwrap()
        .log_out()
        .await
        .unwrap();
    match laptop.try_sync(2).await {
        Err(Error::Api { code, .. }) => assert_eq!(code, error_code::UNAUTHORIZED),
        other => panic!("expected unauthorized, got {other:?}"),
    }
}

#[tokio::test]
async fn the_health_check_recognizes_a_hostly_server() {
    let base = server!();
    ApiClient::new(&base, None).unwrap().health().await.unwrap();

    // Something answers there, but not as a Hostly server (404 on /health).
    let wrong = ApiClient::new(&format!("{base}/api/v1"), None).unwrap();
    assert!(matches!(wrong.health().await, Err(Error::NotASyncServer)));

    let unreachable = ApiClient::new("http://127.0.0.1:9", None).unwrap();
    assert!(matches!(unreachable.health().await, Err(Error::Network(_))));
}
