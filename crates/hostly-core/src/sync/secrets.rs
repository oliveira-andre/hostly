//! Moving hosts' secrets between the keychain and sync rounds. Keychain calls
//! block (and can wait on a permission prompt), so the app runs these off the
//! async workers and never while holding a store lock.

use std::sync::Mutex;

use hostly_protocol::{AuthMethod, Kind};
use uuid::Uuid;

use super::{SealedSecrets, SyncFile};
use crate::Result;
use crate::hosts::HostStore;
use crate::secrets::{SecretStore, host_secret_key};
use crate::vault::VaultKey;

/// Hosts whose secrets the next round needs: pending secret changes, and
/// before the first merge every host that can have one.
pub fn wanted(hosts: &Mutex<HostStore>, sync: &Mutex<SyncFile>) -> Vec<Uuid> {
    let hosts = hosts.lock().unwrap();
    let file = sync.lock().unwrap();
    let mut ids: Vec<Uuid> = file
        .state()
        .pending
        .iter()
        .filter(|(key, change)| key.kind == Kind::Secret && !change.deleted)
        .map(|(key, _)| key.id)
        .collect();
    if file.login().is_some_and(|login| !login.merged) {
        ids.extend(
            hosts
                .list()
                .iter()
                .filter(|host| host.auth != AuthMethod::Agent)
                .map(|host| host.id),
        );
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Reads and seals each secret. One that can't be read is left out and waits
/// for a later round.
pub fn seal(ids: &[Uuid], keychain: &dyn SecretStore, vault: &VaultKey) -> SealedSecrets {
    ids.iter()
        .filter_map(|&id| {
            let secret = keychain.get(&host_secret_key(id)).ok()?;
            let sealed = match secret {
                Some(secret) => Some(vault.seal_secret(id, &secret).ok()?),
                None => None,
            };
            Some((id, sealed))
        })
        .collect()
}

/// Writes received secrets to the keychain and returns how many it wrote. One
/// whose keychain write fails stays in the inbox for the next round; one that
/// doesn't decrypt with `vault` never will, and is dropped.
pub fn deliver(
    sync: &Mutex<SyncFile>,
    keychain: &dyn SecretStore,
    vault: &VaultKey,
) -> Result<usize> {
    let inbox = sync.lock().unwrap().state().inbox.clone();
    let mut written = 0;
    for (id, sealed) in inbox {
        let key = host_secret_key(id);
        let outcome = match &sealed {
            Some(sealed) => match vault.open_secret(id, sealed) {
                Ok(secret) => keychain.set(&key, &secret).map(|()| true),
                Err(_) => Ok(false),
            },
            None => keychain.delete(&key).map(|()| true),
        };
        if let Ok(wrote) = outcome {
            sync.lock().unwrap().take_from_inbox(id, &sealed)?;
            written += usize::from(wrote);
        }
    }
    Ok(written)
}
