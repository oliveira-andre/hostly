//! Hostly's UI-independent core: saved hosts, secret storage, host-key
//! verification, interactive SSH sessions and SFTP.

pub mod error;
pub mod files;
pub mod hosts;
pub mod known_hosts;
pub mod local_fs;
pub mod secrets;
pub mod settings;
pub mod sftp;
pub mod ssh;
mod storage;
pub mod sync;

pub use error::{Error, HostKeyInfo, Result};
