use std::path::PathBuf;

use hostly_protocol::error_code;
use russh::keys::{HashAlg, PublicKey};
use serde::Serialize;
use uuid::Uuid;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("host {0} not found")]
    HostNotFound(Uuid),
    #[error("invalid host: {0}")]
    InvalidHost(String),
    #[error("{kind} {id} not found")]
    NotFound { kind: &'static str, id: Uuid },
    #[error("invalid {kind}: {reason}")]
    InvalidItem { kind: &'static str, reason: String },
    #[error("{}:{} presented an unknown host key ({})", .0.host, .0.port, .0.fingerprint)]
    UnknownHostKey(Box<HostKeyInfo>),
    #[error(
        "the host key for {}:{} has changed and no longer matches {}:{line}; someone could be intercepting this connection",
        info.host, info.port, file.display()
    )]
    HostKeyChanged {
        info: Box<HostKeyInfo>,
        file: PathBuf,
        line: usize,
    },
    #[error("host certificates are not supported yet")]
    UnsupportedHostCertificate,
    #[error("authentication failed")]
    AuthFailed,
    #[error("a password is required")]
    PasswordRequired,
    #[error("the private key is encrypted; a passphrase is required")]
    PassphraseRequired,
    #[error("ssh-agent: {0}")]
    Agent(String),
    #[error("connection timed out")]
    Timeout,
    #[error("the session has ended")]
    SessionClosed,
    #[error("cancelled")]
    Cancelled,
    /// A local or dynamic forward couldn't listen; `address` is `host:port`.
    #[error("couldn't listen on {address}: {source}")]
    ForwardBind {
        address: String,
        source: std::io::Error,
    },
    /// The server declined a remote forward's `tcpip-forward` request.
    #[error(
        "the server refused to listen on {address}; it may not allow port forwarding, or the port is in use"
    )]
    ForwardRefused { address: String },
    #[error("couldn't start the terminal: {0}")]
    Pty(String),
    #[error("{0}")]
    Serial(String),
    #[error(
        "mosh isn't installed on this computer; install it with `brew install mosh` or `apt install mosh`"
    )]
    MoshNotInstalled,
    #[error("mosh-server isn't installed on the host, or isn't on its PATH")]
    MoshServerMissing,
    /// Starting `mosh-server` failed; the message includes what it printed.
    #[error("mosh-server didn't start: {0}")]
    Mosh(String),
    #[error("{0}")]
    Sftp(#[from] russh_sftp::client::error::Error),
    #[error("keychain: {0}")]
    Keychain(String),
    #[error("the server address must start with http:// or https://")]
    InvalidServerUrl,
    #[error("couldn't reach the sync server: {0}")]
    Network(String),
    /// A sync server answered with an error; `code` is a `hostly_protocol::error_code`.
    #[error("{message}")]
    Api {
        status: u16,
        code: String,
        message: String,
    },
    #[error("not logged in")]
    NotLoggedIn,
    /// Deriving keys, or sealing and opening synced secrets, failed.
    #[error("{0}")]
    Vault(String),
    #[error("set your sync server in Settings first")]
    NoSyncServer,
    #[error("this address doesn't answer like a Hostly sync server")]
    NotASyncServer,
    #[error(transparent)]
    Ssh(#[from] russh::Error),
    #[error(transparent)]
    Key(#[from] russh::keys::Error),
    #[error(transparent)]
    SshKey(#[from] russh::keys::ssh_key::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl Error {
    /// Stable identifier the UI uses to decide how to react (prompt, confirm, or show).
    pub fn kind(&self) -> &'static str {
        match self {
            Error::HostNotFound(_) => "hostNotFound",
            Error::InvalidHost(_) => "invalidHost",
            Error::NotFound { .. } => "notFound",
            Error::InvalidItem { .. } => "invalidItem",
            Error::UnknownHostKey(_) => "unknownHostKey",
            Error::HostKeyChanged { .. } => "hostKeyChanged",
            Error::UnsupportedHostCertificate => "unsupportedHostCertificate",
            Error::AuthFailed => "authFailed",
            Error::PasswordRequired => "passwordRequired",
            Error::PassphraseRequired => "passphraseRequired",
            Error::Agent(_) => "agent",
            Error::Timeout => "timeout",
            Error::SessionClosed => "sessionClosed",
            Error::Cancelled => "cancelled",
            Error::ForwardBind { .. } => "forwardBind",
            Error::ForwardRefused { .. } => "forwardRefused",
            Error::Pty(_) => "pty",
            Error::Serial(_) => "serial",
            Error::MoshNotInstalled => "moshNotInstalled",
            Error::MoshServerMissing => "moshServerMissing",
            Error::Mosh(_) => "mosh",
            Error::Sftp(_) => "sftp",
            Error::Keychain(_) => "keychain",
            Error::InvalidServerUrl => "invalidServerUrl",
            Error::Network(_) => "network",
            Error::Api { code, .. } => match code.as_str() {
                error_code::INVALID => "invalid",
                error_code::EMAIL_TAKEN => "emailTaken",
                error_code::INVALID_CREDENTIALS => "invalidCredentials",
                error_code::PLAIN_PASSWORD => "plainPassword",
                error_code::UNAUTHORIZED => "unauthorized",
                error_code::TOO_MANY_ATTEMPTS => "tooManyAttempts",
                error_code::ACCOUNT_LOCKED => "accountLocked",
                _ => "api",
            },
            Error::NotLoggedIn => "notLoggedIn",
            Error::Vault(_) => "vault",
            Error::NoSyncServer => "noSyncServer",
            Error::NotASyncServer => "notASyncServer",
            Error::Ssh(_) | Error::Key(_) | Error::SshKey(_) => "ssh",
            Error::Io(_) => "io",
            Error::Json(_) => "storage",
        }
    }

    pub fn host_key(&self) -> Option<&HostKeyInfo> {
        match self {
            Error::UnknownHostKey(info) | Error::HostKeyChanged { info, .. } => Some(info.as_ref()),
            _ => None,
        }
    }
}

/// A server's host key, in the forms the user needs to verify and trust it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyInfo {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:…`, the same format `ssh-keygen -lf` prints.
    pub fingerprint: String,
    /// The key in OpenSSH `known_hosts` format, used to trust it.
    pub public_key: String,
}

impl HostKeyInfo {
    pub fn new(host: &str, port: u16, key: &PublicKey) -> Result<Self> {
        Ok(Self {
            host: host.to_owned(),
            port,
            algorithm: key.algorithm().as_str().to_owned(),
            fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            public_key: key.to_openssh()?,
        })
    }
}
