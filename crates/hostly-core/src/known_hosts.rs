use std::path::PathBuf;

use russh::keys::{self, PublicKey, known_hosts};

use crate::Result;

/// Host-key trust. OpenSSH's own `~/.ssh/known_hosts` is honoured but never
/// written; keys approved in Hostly are recorded in `app_file`.
#[derive(Clone, Debug)]
pub struct KnownHosts {
    pub user_file: Option<PathBuf>,
    pub app_file: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HostKeyStatus {
    Trusted,
    Unknown,
    Changed { file: PathBuf, line: usize },
}

impl KnownHosts {
    pub fn check(&self, host: &str, port: u16, key: &PublicKey) -> Result<HostKeyStatus> {
        let mut trusted = false;
        for file in self.user_file.iter().chain([&self.app_file]) {
            match known_hosts::check_known_hosts_path(host, port, key, file) {
                Ok(found) => trusted |= found,
                // A recorded key of the same type that doesn't match: refuse,
                // even if another file trusts the new key.
                Err(keys::Error::KeyChanged { line }) => {
                    return Ok(HostKeyStatus::Changed {
                        file: file.clone(),
                        line,
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(if trusted {
            HostKeyStatus::Trusted
        } else {
            HostKeyStatus::Unknown
        })
    }

    pub fn trust(&self, host: &str, port: u16, key: &PublicKey) -> Result<()> {
        known_hosts::learn_known_hosts_path(host, port, key, &self.app_file)?;
        Ok(())
    }

    /// Trusts a key in OpenSSH format, as carried by [`crate::HostKeyInfo::public_key`].
    pub fn trust_openssh(&self, host: &str, port: u16, public_key: &str) -> Result<()> {
        self.trust(host, port, &PublicKey::from_openssh(public_key)?)
    }
}
