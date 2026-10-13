use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::storage::{read_json, write_json};
use crate::{Error, Result};

const FILE_VERSION: u32 = 1;

pub use hostly_protocol::{AuthMethod, Host};

/// Expands a leading `~` so key paths can be entered the way users type them in a shell.
pub fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

#[derive(Serialize, Deserialize)]
struct HostFile {
    version: u32,
    hosts: Vec<Host>,
}

/// Saved hosts, persisted as JSON. Every change is written to disk before it
/// becomes visible, so memory and disk never disagree.
pub struct HostStore {
    path: PathBuf,
    hosts: Vec<Host>,
}

impl HostStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let hosts = read_json::<HostFile>(&path)?.map_or_else(Vec::new, |file| file.hosts);
        Ok(Self { path, hosts })
    }

    pub fn list(&self) -> &[Host] {
        &self.hosts
    }

    pub fn get(&self, id: Uuid) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// Adds a new host or replaces the saved host with the same id.
    pub fn upsert(&mut self, host: Host) -> Result<Host> {
        host.validate().map_err(Error::InvalidHost)?;
        let mut hosts = self.hosts.clone();
        match hosts.iter_mut().find(|h| h.id == host.id) {
            Some(existing) => *existing = host.clone(),
            None => hosts.push(host.clone()),
        }
        self.commit(hosts)?;
        Ok(host)
    }

    pub fn remove(&mut self, id: Uuid) -> Result<Host> {
        let index = self
            .hosts
            .iter()
            .position(|h| h.id == id)
            .ok_or(Error::HostNotFound(id))?;
        let mut hosts = self.hosts.clone();
        let removed = hosts.remove(index);
        self.commit(hosts)?;
        Ok(removed)
    }

    /// Applies changes received from the sync server in a single write.
    /// Hosts that fail validation are skipped. Returns whether anything changed.
    pub fn apply_remote(&mut self, upserts: Vec<Host>, removals: &[Uuid]) -> Result<bool> {
        let mut hosts = self.hosts.clone();
        let mut changed = false;
        for host in upserts {
            if host.validate().is_err() {
                continue;
            }
            match hosts.iter_mut().find(|h| h.id == host.id) {
                Some(existing) if *existing == host => {}
                Some(existing) => {
                    *existing = host;
                    changed = true;
                }
                None => {
                    hosts.push(host);
                    changed = true;
                }
            }
        }
        let before = hosts.len();
        hosts.retain(|h| !removals.contains(&h.id));
        changed |= hosts.len() != before;
        if changed {
            self.commit(hosts)?;
        }
        Ok(changed)
    }

    fn commit(&mut self, hosts: Vec<Host>) -> Result<()> {
        let file = HostFile {
            version: FILE_VERSION,
            hosts,
        };
        write_json(&self.path, &file)?;
        self.hosts = file.hosts;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(label: &str) -> Host {
        Host {
            id: Uuid::new_v4(),
            label: label.into(),
            hostname: "example.com".into(),
            port: 22,
            username: "deploy".into(),
            auth: AuthMethod::Agent,
            group: None,
        }
    }

    #[test]
    fn upsert_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.json");
        let mut store = HostStore::open(&path).unwrap();
        let a = store.upsert(host("a")).unwrap();
        let mut b = store.upsert(host("b")).unwrap();
        b.auth = AuthMethod::Key {
            path: "~/.ssh/id_ed25519".into(),
        };
        store.upsert(b.clone()).unwrap();

        let reloaded = HostStore::open(&path).unwrap();
        assert_eq!(reloaded.list(), &[a, b]);
    }

    #[test]
    fn remove_deletes_and_reports_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = HostStore::open(dir.path().join("hosts.json")).unwrap();
        let a = store.upsert(host("a")).unwrap();
        assert_eq!(store.remove(a.id).unwrap(), a);
        assert!(store.list().is_empty());
        assert!(matches!(store.remove(a.id), Err(Error::HostNotFound(_))));
    }

    #[test]
    fn invalid_host_is_rejected_without_touching_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hosts.json");
        let mut store = HostStore::open(&path).unwrap();
        let mut bad = host("bad");
        bad.hostname = "two words".into();
        assert!(matches!(store.upsert(bad), Err(Error::InvalidHost(_))));
        assert!(!path.exists());
    }

    #[test]
    fn new_host_from_ui_gets_an_id_and_default_port() {
        let json = r#"{"hostname":"10.0.0.5","username":"pi","auth":{"type":"password"}}"#;
        let host: Host = serde_json::from_str(json).unwrap();
        assert_eq!(host.port, 22);
        assert!(!host.id.is_nil());
        assert_eq!(host.auth, AuthMethod::Password);
    }

    #[test]
    fn expands_leading_tilde_only() {
        let home = Path::new("/Users/me");
        assert_eq!(
            expand_home(Path::new("~/.ssh/id_ed25519"), Some(home)),
            PathBuf::from("/Users/me/.ssh/id_ed25519")
        );
        assert_eq!(
            expand_home(Path::new("/keys/~id"), Some(home)),
            PathBuf::from("/keys/~id")
        );
    }
}
