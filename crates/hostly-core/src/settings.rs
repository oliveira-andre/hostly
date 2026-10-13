//! App settings other than hosts: currently the sync server address.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::storage::{read_json, write_json};
use crate::sync::ApiClient;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// The sync server chosen in the app. `None` falls back to the app's built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_server: Option<String>,
}

/// Settings, persisted as JSON in the app data directory.
pub struct SettingsFile {
    path: PathBuf,
    settings: Settings,
}

impl SettingsFile {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let settings = read_json(&path)?.unwrap_or_default();
        Ok(Self { path, settings })
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Saves the sync server address, normalized. `None` or a blank address
    /// returns to the built-in default.
    pub fn set_sync_server(&mut self, server: Option<&str>) -> Result<()> {
        let sync_server = match server.map(str::trim).filter(|s| !s.is_empty()) {
            Some(server) => Some(ApiClient::normalize_server(server)?),
            None => None,
        };
        let settings = Settings { sync_server };
        write_json(&self.path, &settings)?;
        self.settings = settings;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn sync_server_is_normalized_persisted_and_clearable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut file = SettingsFile::open(&path).unwrap();
        assert_eq!(file.settings().sync_server, None);

        file.set_sync_server(Some(" https://sync.example.com/ "))
            .unwrap();
        let reopened = SettingsFile::open(&path).unwrap();
        assert_eq!(
            reopened.settings().sync_server.as_deref(),
            Some("https://sync.example.com")
        );

        file.set_sync_server(Some("  ")).unwrap();
        assert_eq!(
            SettingsFile::open(&path).unwrap().settings().sync_server,
            None
        );
    }

    #[test]
    fn an_invalid_address_is_rejected_and_nothing_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = SettingsFile::open(dir.path().join("settings.json")).unwrap();
        file.set_sync_server(Some("http://localhost:8080")).unwrap();
        assert!(matches!(
            file.set_sync_server(Some("sync.example.com")),
            Err(Error::InvalidServerUrl)
        ));
        assert_eq!(
            file.settings().sync_server.as_deref(),
            Some("http://localhost:8080")
        );
    }
}
