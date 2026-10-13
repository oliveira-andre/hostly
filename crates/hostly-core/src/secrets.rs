use std::collections::HashMap;
use std::sync::Mutex;

use uuid::Uuid;

use crate::{Error, Result};

/// Where passwords and key passphrases are kept, outside the hosts file.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>>;
    fn set(&self, key: &str, secret: &str) -> Result<()>;
    /// Succeeds when there is nothing to delete.
    fn delete(&self, key: &str) -> Result<()>;
}

pub fn host_secret_key(id: Uuid) -> String {
    format!("host/{id}")
}

/// The OS credential store: macOS Keychain, Windows Credential Manager, or
/// the Secret Service on Linux.
pub struct Keychain {
    service: String,
}

impl Keychain {
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key).map_err(keychain_error)
    }
}

impl SecretStore for Keychain {
    fn get(&self, key: &str) -> Result<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keychain_error(e)),
        }
    }

    fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.entry(key)?
            .set_password(secret)
            .map_err(keychain_error)
    }

    fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keychain_error(e)),
        }
    }
}

fn keychain_error(e: keyring::Error) -> Error {
    Error::Keychain(e.to_string())
}

/// In-memory store for tests and for running without an OS keychain.
#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, secret: &str) -> Result<()> {
        self.0.lock().unwrap().insert(key.into(), secret.into());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
