use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use hostly_core::hosts::HostStore;
use hostly_core::known_hosts::KnownHosts;
use hostly_core::secrets::{Keychain, SecretStore, host_secret_key};
use hostly_core::settings::SettingsFile;
use hostly_core::sftp::SftpClient;
use hostly_core::ssh::SessionHandle;
use hostly_core::store::{ForwardStore, SnippetStore};
use hostly_core::sync::{Local, SyncFile};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::account::SyncService;
use crate::forwarding::Run;

/// Data directory name and keychain service from before the app was renamed to Hostly.
const LEGACY_IDENTIFIER: &str = "app.ferrite.desktop";

pub struct AppState {
    pub hosts: Mutex<HostStore>,
    pub snippets: Mutex<SnippetStore>,
    pub forwards: Mutex<ForwardStore>,
    pub secrets: Arc<dyn SecretStore>,
    /// Held across a host secret's keychain write and the sync record that
    /// follows it, so a secret arriving from sync can't overwrite a newer one.
    pub secret_writes: tokio::sync::Mutex<()>,
    pub known_hosts: KnownHosts,
    pub sessions: Mutex<HashMap<Uuid, SessionHandle>>,
    pub sftp: Mutex<HashMap<Uuid, Arc<SftpClient>>>,
    /// Cancel flags of running transfers.
    pub transfers: Mutex<HashMap<Uuid, Arc<AtomicBool>>>,
    /// Port-forwarding rules that are starting, running or stopped with an error.
    pub forwarding: Mutex<HashMap<Uuid, Run>>,
    pub home_dir: Option<PathBuf>,
    pub sync: SyncService,
    pub settings: Mutex<SettingsFile>,
}

impl AppState {
    /// Everything lives in the per-user app data directory, e.g.
    /// `~/Library/Application Support/app.hostly.desktop` on macOS.
    pub fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = app.path().app_data_dir()?;
        let secrets = Arc::new(Keychain::new(&app.config().identifier));
        migrate_legacy_data(
            &data_dir.with_file_name(LEGACY_IDENTIFIER),
            &data_dir,
            &Keychain::new(LEGACY_IDENTIFIER),
            secrets.as_ref(),
        )?;
        std::fs::create_dir_all(&data_dir)?;
        Ok(Self::open(&data_dir, secrets, app.path().home_dir().ok())?)
    }

    /// The state kept in `data_dir`, with secrets in `secrets`.
    pub fn open(
        data_dir: &Path,
        secrets: Arc<dyn SecretStore>,
        home_dir: Option<PathBuf>,
    ) -> hostly_core::Result<Self> {
        Ok(Self {
            hosts: Mutex::new(HostStore::open(data_dir.join("hosts.json"))?),
            snippets: Mutex::new(SnippetStore::open(data_dir.join("snippets.json"))?),
            forwards: Mutex::new(ForwardStore::open(data_dir.join("forwards.json"))?),
            secrets,
            secret_writes: tokio::sync::Mutex::const_new(()),
            known_hosts: KnownHosts {
                user_file: home_dir.as_ref().map(|home| home.join(".ssh/known_hosts")),
                app_file: data_dir.join("known_hosts"),
            },
            sessions: Mutex::default(),
            sftp: Mutex::default(),
            transfers: Mutex::default(),
            forwarding: Mutex::default(),
            home_dir,
            sync: SyncService::new(SyncFile::open(data_dir.join("sync.json"))?),
            settings: Mutex::new(SettingsFile::open(data_dir.join("settings.json"))?),
        })
    }
}

impl AppState {
    /// The stores sync keeps up to date.
    pub fn local(&self) -> Local<'_> {
        Local {
            hosts: &self.hosts,
            snippets: &self.snippets,
            forwards: &self.forwards,
        }
    }
}

/// One-time move of hosts, approved host keys and saved secrets from the
/// pre-rename location. Runs only while the new data directory doesn't exist yet.
fn migrate_legacy_data(
    legacy_dir: &Path,
    data_dir: &Path,
    legacy_secrets: &dyn SecretStore,
    secrets: &dyn SecretStore,
) -> std::io::Result<()> {
    if data_dir.exists() || !legacy_dir.is_dir() {
        return Ok(());
    }
    std::fs::rename(legacy_dir, data_dir)?;

    let Ok(hosts) = HostStore::open(data_dir.join("hosts.json")) else {
        return Ok(());
    };
    for host in hosts.list() {
        let key = host_secret_key(host.id);
        // Best effort: a secret that can't be moved (e.g. the keychain prompt was
        // denied) is simply asked for again on the next connection.
        if let Ok(Some(secret)) = legacy_secrets.get(&key)
            && secrets.set(&key, &secret).is_ok()
        {
            let _ = legacy_secrets.delete(&key);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use hostly_core::hosts::{AuthMethod, Host};
    use hostly_core::secrets::MemoryStore;

    use super::*;

    fn legacy_install(root: &Path) -> (PathBuf, Host, MemoryStore) {
        let legacy_dir = root.join(LEGACY_IDENTIFIER);
        let mut store = HostStore::open(legacy_dir.join("hosts.json")).unwrap();
        let host = store
            .upsert(Host {
                id: Uuid::new_v4(),
                label: "db".into(),
                hostname: "db.example.com".into(),
                port: 22,
                username: "deploy".into(),
                auth: AuthMethod::Password,
                group: None,
                mosh: false,
            })
            .unwrap();
        std::fs::write(
            legacy_dir.join("known_hosts"),
            "db.example.com ssh-ed25519 AAAA\n",
        )
        .unwrap();
        let legacy_secrets = MemoryStore::default();
        legacy_secrets
            .set(&host_secret_key(host.id), "hunter2")
            .unwrap();
        (legacy_dir, host, legacy_secrets)
    }

    #[test]
    fn moves_hosts_known_hosts_and_secrets() {
        let root = tempfile::tempdir().unwrap();
        let (legacy_dir, host, legacy_secrets) = legacy_install(root.path());
        let data_dir = root.path().join("app.hostly.desktop");
        let secrets = MemoryStore::default();

        migrate_legacy_data(&legacy_dir, &data_dir, &legacy_secrets, &secrets).unwrap();

        assert!(!legacy_dir.exists());
        assert_eq!(
            HostStore::open(data_dir.join("hosts.json")).unwrap().list(),
            std::slice::from_ref(&host)
        );
        assert!(data_dir.join("known_hosts").exists());
        let key = host_secret_key(host.id);
        assert_eq!(secrets.get(&key).unwrap().as_deref(), Some("hunter2"));
        assert_eq!(legacy_secrets.get(&key).unwrap(), None);
    }

    #[test]
    fn leaves_everything_alone_once_the_new_directory_exists() {
        let root = tempfile::tempdir().unwrap();
        let (legacy_dir, host, legacy_secrets) = legacy_install(root.path());
        let data_dir = root.path().join("app.hostly.desktop");
        std::fs::create_dir(&data_dir).unwrap();
        let secrets = MemoryStore::default();

        migrate_legacy_data(&legacy_dir, &data_dir, &legacy_secrets, &secrets).unwrap();

        assert!(legacy_dir.join("hosts.json").exists());
        assert!(!data_dir.join("hosts.json").exists());
        assert!(
            legacy_secrets
                .get(&host_secret_key(host.id))
                .unwrap()
                .is_some()
        );
        assert_eq!(secrets.get(&host_secret_key(host.id)).unwrap(), None);
    }

    #[test]
    fn fresh_install_without_legacy_data_is_a_no_op() {
        let root = tempfile::tempdir().unwrap();
        let data_dir = root.path().join("app.hostly.desktop");
        let none = MemoryStore::default();
        migrate_legacy_data(
            &root.path().join(LEGACY_IDENTIFIER),
            &data_dir,
            &none,
            &none,
        )
        .unwrap();
        assert!(!data_dir.exists());
    }
}
