use std::path::{Path, PathBuf};

use uuid::Uuid;

use crate::store::{ItemStore, Stored};
use crate::{Error, Result};

pub use hostly_protocol::{AuthMethod, Host};

/// Expands a leading `~` so key paths can be entered the way users type them in a shell.
pub fn expand_home(path: &Path, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~"), home) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => path.to_path_buf(),
    }
}

/// Saved hosts, in `hosts.json`.
pub type HostStore = ItemStore<Host>;

impl Stored for Host {
    const FIELD: &'static str = "hosts";

    fn id(&self) -> Uuid {
        self.id
    }

    fn check(&self) -> Result<()> {
        self.validate().map_err(Error::InvalidHost)
    }

    fn not_found(id: Uuid) -> Error {
        Error::HostNotFound(id)
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
            mosh: false,
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
