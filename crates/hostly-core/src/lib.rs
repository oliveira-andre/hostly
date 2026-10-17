//! Hostly's UI-independent core: saved hosts, snippets and port-forwarding
//! rules, secret storage, host-key verification, interactive SSH sessions,
//! SFTP, port forwarding, local terminals, serial ports, Mosh, and sync with
//! end-to-end encrypted secrets.

pub mod error;
pub mod files;
pub mod forward;
pub mod hosts;
pub mod known_hosts;
pub mod local_fs;
pub mod mosh;
pub mod pty;
pub mod secrets;
pub mod serial;
pub mod session;
pub mod settings;
pub mod sftp;
pub mod ssh;
mod storage;
pub mod store;
pub mod sync;
pub mod vault;

pub use error::{Error, HostKeyInfo, Result};
