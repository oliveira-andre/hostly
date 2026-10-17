//! Saved items (hosts, snippets, port-forwarding rules), each kind in its own
//! versioned JSON file: `{"version": 1, "<field>": [...]}`.

use std::path::PathBuf;

use hostly_protocol::{Forward, Snippet};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::storage::{read_json, write_json};
use crate::{Error, Result};

const FILE_VERSION: u32 = 1;

/// An item kept in an [`ItemStore`].
pub trait Stored: Clone + PartialEq + Serialize + DeserializeOwned {
    /// The array's field in the file, e.g. `hosts`.
    const FIELD: &'static str;
    fn id(&self) -> Uuid;
    fn check(&self) -> Result<()>;
    fn not_found(id: Uuid) -> Error;
}

impl Stored for Snippet {
    const FIELD: &'static str = "snippets";

    fn id(&self) -> Uuid {
        self.id
    }

    fn check(&self) -> Result<()> {
        self.validate().map_err(|reason| Error::InvalidItem {
            kind: "snippet",
            reason,
        })
    }

    fn not_found(id: Uuid) -> Error {
        Error::NotFound {
            kind: "snippet",
            id,
        }
    }
}

impl Stored for Forward {
    const FIELD: &'static str = "forwards";

    fn id(&self) -> Uuid {
        self.id
    }

    fn check(&self) -> Result<()> {
        self.validate().map_err(|reason| Error::InvalidItem {
            kind: "port forward",
            reason,
        })
    }

    fn not_found(id: Uuid) -> Error {
        Error::NotFound {
            kind: "port forward",
            id,
        }
    }
}

pub type SnippetStore = ItemStore<Snippet>;
pub type ForwardStore = ItemStore<Forward>;

/// Saved items persisted as JSON. Every change is written to disk before it
/// becomes visible, so memory and disk never disagree.
pub struct ItemStore<T> {
    path: PathBuf,
    items: Vec<T>,
}

impl<T: Stored> ItemStore<T> {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let items = match read_json::<serde_json::Value>(&path)? {
            Some(mut file) => match file.get_mut(T::FIELD).map(serde_json::Value::take) {
                Some(items) => serde_json::from_value(items)?,
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        Ok(Self { path, items })
    }

    pub fn list(&self) -> &[T] {
        &self.items
    }

    pub fn get(&self, id: Uuid) -> Option<&T> {
        self.items.iter().find(|item| item.id() == id)
    }

    /// Adds a new item or replaces the saved item with the same id.
    pub fn upsert(&mut self, item: T) -> Result<T> {
        item.check()?;
        let mut items = self.items.clone();
        match items.iter_mut().find(|i| i.id() == item.id()) {
            Some(existing) => *existing = item.clone(),
            None => items.push(item.clone()),
        }
        self.commit(items)?;
        Ok(item)
    }

    pub fn remove(&mut self, id: Uuid) -> Result<T> {
        let index = self
            .items
            .iter()
            .position(|i| i.id() == id)
            .ok_or_else(|| T::not_found(id))?;
        let mut items = self.items.clone();
        let removed = items.remove(index);
        self.commit(items)?;
        Ok(removed)
    }

    /// Applies changes received from the sync server in a single write.
    /// Items that fail validation are skipped. Returns whether anything changed.
    pub fn apply_remote(&mut self, upserts: Vec<T>, removals: &[Uuid]) -> Result<bool> {
        let mut items = self.items.clone();
        let mut changed = false;
        for item in upserts {
            if item.check().is_err() {
                continue;
            }
            match items.iter_mut().find(|i| i.id() == item.id()) {
                Some(existing) if *existing == item => {}
                Some(existing) => {
                    *existing = item;
                    changed = true;
                }
                None => {
                    items.push(item);
                    changed = true;
                }
            }
        }
        let before = items.len();
        items.retain(|i| !removals.contains(&i.id()));
        changed |= items.len() != before;
        if changed {
            self.commit(items)?;
        }
        Ok(changed)
    }

    fn commit(&mut self, items: Vec<T>) -> Result<()> {
        let file = serde_json::json!({
            "version": FILE_VERSION,
            T::FIELD: items,
        });
        write_json(&self.path, &file)?;
        self.items = items;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use hostly_protocol::ForwardKind;

    use super::*;

    fn snippet(name: &str) -> Snippet {
        Snippet {
            id: Uuid::new_v4(),
            name: name.into(),
            script: "uptime".into(),
        }
    }

    #[test]
    fn snippets_persist_and_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snippets.json");
        let mut store = SnippetStore::open(&path).unwrap();
        let a = store.upsert(snippet("a")).unwrap();
        let mut b = store.upsert(snippet("b")).unwrap();
        b.script = "df -h\nfree -m".into();
        store.upsert(b.clone()).unwrap();
        store.remove(a.id).unwrap();

        let reloaded = SnippetStore::open(&path).unwrap();
        assert_eq!(reloaded.list(), std::slice::from_ref(&b));
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
        assert!(json["snippets"].is_array());
    }

    #[test]
    fn invalid_items_are_rejected_and_missing_ones_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forwards.json");
        let mut store = ForwardStore::open(&path).unwrap();
        let rule = Forward {
            id: Uuid::new_v4(),
            label: "db".into(),
            host_id: Uuid::new_v4(),
            kind: ForwardKind::Local,
            bind_address: "127.0.0.1".into(),
            bind_port: 5433,
            destination_host: None,
            destination_port: Some(5432),
        };
        assert!(matches!(
            store.upsert(rule.clone()),
            Err(Error::InvalidItem { .. })
        ));
        assert!(!path.exists());
        assert!(matches!(store.remove(rule.id), Err(Error::NotFound { .. })));
    }

    #[test]
    fn remote_changes_skip_invalid_items() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = SnippetStore::open(dir.path().join("snippets.json")).unwrap();
        let kept = store.upsert(snippet("kept")).unwrap();
        let mut bad = snippet("bad");
        bad.script = " ".into();
        let added = snippet("added");
        assert!(
            store
                .apply_remote(vec![bad, added.clone()], &[kept.id])
                .unwrap()
        );
        assert_eq!(store.list(), &[added]);
        assert!(!store.apply_remote(Vec::new(), &[Uuid::new_v4()]).unwrap());
    }
}
