//! Types shared by the Hostly app and the Hostly sync server: the synced
//! models (hosts, snippets, port forwards, sealed secrets) and the JSON API.
//! All JSON is camelCase.
//!
//! # API (`/api/v1`)
//!
//! | Method and path         | Body                 | Success                  | Auth   |
//! |-------------------------|----------------------|--------------------------|--------|
//! | `POST /accounts`        | [`NewAccount`]       | 201 [`Session`]          | –      |
//! | `POST /sessions`        | [`Credentials`]      | 200 [`Session`]          | –      |
//! | `DELETE /sessions/current` | –                 | 204                      | Bearer |
//! | `GET /account`          | –                    | 200 [`Account`]          | Bearer |
//! | `PUT /account/password` | [`PasswordChange`]   | 204                      | Bearer |
//! | `GET /items?since=N`    | –                    | 200 [`ItemChanges`]      | Bearer |
//! | `POST /items`           | [`ItemPushRequest`]  | 200 [`ItemPushResponse`] | Bearer |
//! | `GET /hosts?since=N`    | –                    | 200 [`Changes`]          | Bearer |
//! | `POST /hosts`           | [`PushRequest`]      | 200 [`PushResponse`]     | Bearer |
//!
//! `/items` syncs every [`Kind`] through one change log. `/hosts` is the same
//! log limited to hosts, kept for apps from before snippets and port forwards.
//!
//! `GET /health` (outside `/api/v1`) answers 200 `ok`. Failures answer with an
//! [`ApiError`] body; see [`error_code`] for the codes.
//!
//! `POST /sessions` is limited: too many failed log-ins from one address, or
//! in a row for one account, are refused for a while (429
//! [`error_code::TOO_MANY_ATTEMPTS`], 423 [`error_code::ACCOUNT_LOCKED`]). Both
//! carry a `Retry-After` header in seconds.
//!
//! # End-to-end encryption
//!
//! Apps derive two keys from the account password on the device: a login key,
//! sent as [`Credentials::password`] with [`KEY_VERSION_DERIVED`], and a
//! wrapping key that never leaves the device. A random vault key encrypts
//! synced passwords and passphrases ([`Kind::Secret`]); the server only stores
//! it wrapped ([`Session::vault_key`]), so it can't read any secret. Changing
//! the password re-wraps the same vault key. Accounts created by older apps
//! log in with the password as typed ([`KEY_VERSION_PLAIN`]) until an app
//! upgrades them through `PUT /account/password`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const API_PREFIX: &str = "/api/v1";

/// Minimum account password length. The server enforces it on what it
/// receives; apps check the typed password before deriving a login key.
pub const MIN_PASSWORD_LEN: usize = 8;

/// [`Credentials::password`] is the password as typed (apps from before
/// end-to-end encryption, and accounts they created).
pub const KEY_VERSION_PLAIN: u8 = 0;
/// [`Credentials::password`] is a login key derived from the password on the device.
pub const KEY_VERSION_DERIVED: u8 = 1;

/// Upper bounds the server enforces, so one item can't bloat an account.
pub const MAX_SNIPPET_LEN: usize = 64 * 1024;
pub const MAX_SEALED_LEN: usize = 32 * 1024;

/// A saved connection. Secrets (passwords, key passphrases) are never part of
/// it: they stay in each device's keychain and sync only as [`Kind::Secret`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Host {
    /// A host submitted without an id is a new host and gets a fresh one.
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    #[serde(default)]
    pub label: String,
    pub hostname: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub username: String,
    pub auth: AuthMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Connect with Mosh (bootstrapped over SSH) instead of an SSH shell.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mosh: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthMethod {
    Password,
    Key { path: PathBuf },
    Agent,
}

fn default_port() -> u16 {
    22
}

fn is_false(value: &bool) -> bool {
    !value
}

fn is_plain(version: &u8) -> bool {
    *version == KEY_VERSION_PLAIN
}

/// A hostname or address: required, without spaces.
fn check_address(value: &str, what: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{what} is required"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(format!("{what} cannot contain spaces"));
    }
    Ok(())
}

impl Host {
    /// Returns the reason the host can't be saved, if any.
    pub fn validate(&self) -> Result<(), String> {
        check_address(&self.hostname, "hostname")?;
        if self.username.trim().is_empty() {
            return Err("username is required".into());
        }
        if self.port == 0 {
            return Err("port must be between 1 and 65535".into());
        }
        if let AuthMethod::Key { path } = &self.auth
            && path.as_os_str().is_empty()
        {
            return Err("private key path is required".into());
        }
        Ok(())
    }
}

/// A saved command to run in a terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    /// A snippet submitted without an id is new and gets a fresh one.
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    /// Sent to the terminal as typed; each line ends with Enter.
    pub script: String,
}

impl Snippet {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name is required".into());
        }
        if self.script.trim().is_empty() {
            return Err("script is required".into());
        }
        if self.script.len() > MAX_SNIPPET_LEN {
            return Err(format!("script is longer than {MAX_SNIPPET_LEN} bytes"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ForwardKind {
    /// Listen on this device; connections reach the destination from the SSH server.
    Local,
    /// The SSH server listens; connections reach the destination from this device.
    Remote,
    /// A SOCKS proxy on this device; each connection picks its destination.
    Dynamic,
}

/// A saved port-forwarding rule, tunneled through one of the saved hosts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Forward {
    /// A rule submitted without an id is new and gets a fresh one.
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    #[serde(default)]
    pub label: String,
    /// The host whose SSH connection carries the tunnel.
    pub host_id: Uuid,
    pub kind: ForwardKind,
    /// Where to listen: on this device, or on the server for [`ForwardKind::Remote`].
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    pub bind_port: u16,
    /// Required for local and remote rules; dynamic rules have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination_port: Option<u16>,
}

fn default_bind_address() -> String {
    "127.0.0.1".into()
}

impl Forward {
    pub fn validate(&self) -> Result<(), String> {
        check_address(&self.bind_address, "listen address")?;
        if self.bind_port == 0 {
            return Err("listen port must be between 1 and 65535".into());
        }
        match self.kind {
            ForwardKind::Local | ForwardKind::Remote => {
                check_address(
                    self.destination_host.as_deref().unwrap_or(""),
                    "destination host",
                )?;
                if !matches!(self.destination_port, Some(1..)) {
                    return Err("destination port must be between 1 and 65535".into());
                }
            }
            ForwardKind::Dynamic => {
                if self.destination_host.is_some() || self.destination_port.is_some() {
                    return Err("a dynamic (SOCKS) rule has no destination".into());
                }
            }
        }
        Ok(())
    }
}

/// What a synced item is. Items of different kinds may share an id: a host's
/// [`Kind::Secret`] uses the host's id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Host,
    Snippet,
    Forward,
    /// A host's password or key passphrase, sealed with the account's vault
    /// key. Only devices holding that key can read it.
    Secret,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Host, Kind::Snippet, Kind::Forward, Kind::Secret];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Host => "host",
            Kind::Snippet => "snippet",
            Kind::Forward => "forward",
            Kind::Secret => "secret",
        }
    }

    pub fn parse(value: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

/// A synced item's content, as decoded from [`ItemRecord::data`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    Host(Host),
    Snippet(Snippet),
    Forward(Forward),
    /// `v1.` followed by base64url (no padding) of a 24-byte nonce and the
    /// XChaCha20-Poly1305 ciphertext.
    Secret(String),
}

impl Item {
    pub fn kind(&self) -> Kind {
        match self {
            Item::Host(_) => Kind::Host,
            Item::Snippet(_) => Kind::Snippet,
            Item::Forward(_) => Kind::Forward,
            Item::Secret(_) => Kind::Secret,
        }
    }

    /// Decodes and validates `data` as an item of `kind` stored under `id`.
    pub fn decode(kind: Kind, id: Uuid, data: serde_json::Value) -> Result<Item, String> {
        let item = match kind {
            Kind::Host => serde_json::from_value(data).map(Item::Host),
            Kind::Snippet => serde_json::from_value(data).map(Item::Snippet),
            Kind::Forward => serde_json::from_value(data).map(Item::Forward),
            Kind::Secret => serde_json::from_value(data).map(Item::Secret),
        }
        .map_err(|e| format!("{} {id}: {e}", kind.as_str()))?;
        item.validate(id)?;
        Ok(item)
    }

    pub fn encode(&self) -> serde_json::Value {
        let value = match self {
            Item::Host(host) => serde_json::to_value(host),
            Item::Snippet(snippet) => serde_json::to_value(snippet),
            Item::Forward(forward) => serde_json::to_value(forward),
            Item::Secret(sealed) => serde_json::to_value(sealed),
        };
        value.expect("items always serialize")
    }

    /// The reason the item can't be stored under `id`, if any.
    pub fn validate(&self, id: Uuid) -> Result<(), String> {
        let (own_id, result) = match self {
            Item::Host(host) => (Some(host.id), host.validate()),
            Item::Snippet(snippet) => (Some(snippet.id), snippet.validate()),
            Item::Forward(forward) => (Some(forward.id), forward.validate()),
            Item::Secret(sealed) => (None, validate_sealed(sealed)),
        };
        let kind = self.kind().as_str();
        if let Some(own_id) = own_id
            && own_id != id
        {
            return Err(format!("{kind} id {own_id} does not match change id {id}"));
        }
        result.map_err(|reason| format!("{kind} {id}: {reason}"))
    }
}

/// Checks the shape of a sealed secret; only devices can check its content.
pub fn validate_sealed(sealed: &str) -> Result<(), String> {
    let body = sealed
        .strip_prefix("v1.")
        .ok_or("sealed secret has an unknown format")?;
    // A 24-byte nonce and a 16-byte tag at least, base64-encoded.
    let well_formed = body.len() >= 54
        && sealed.len() <= MAX_SEALED_LEN
        && body
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if well_formed {
        Ok(())
    } else {
        Err("sealed secret is malformed".into())
    }
}

/// Sign-up and log-in request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub email: String,
    pub password: String,
    /// How `password` was produced: [`KEY_VERSION_PLAIN`] or [`KEY_VERSION_DERIVED`].
    #[serde(default, skip_serializing_if = "is_plain")]
    pub key_version: u8,
}

/// `POST /accounts`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    #[serde(flatten)]
    pub credentials: Credentials,
    /// The new vault key, wrapped; required with [`KEY_VERSION_DERIVED`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_key: Option<String>,
}

/// `PUT /account/password`: replaces the login key and the wrapped vault key.
/// Also upgrades an account from [`KEY_VERSION_PLAIN`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordChange {
    /// The current password, in the form the account uses now.
    pub current_password: String,
    #[serde(default, skip_serializing_if = "is_plain")]
    pub current_key_version: u8,
    /// The new login key, always [`KEY_VERSION_DERIVED`].
    pub new_password: String,
    /// The account's vault key, wrapped with the key derived from the new password.
    pub vault_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: Uuid,
    pub email: String,
}

/// A logged-in session. `token` goes in `Authorization: Bearer <token>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub token: String,
    pub account: Account,
    /// The account's vault key, wrapped; absent until the account is upgraded
    /// to [`KEY_VERSION_DERIVED`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault_key: Option<String>,
}

/// The server's current state of one synced item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemRecord {
    pub kind: Kind,
    pub id: Uuid,
    /// The item, decoded with [`Item::decode`]; `None` once deleted (a tombstone).
    pub data: Option<serde_json::Value>,
    /// When the change was made, in milliseconds since the Unix epoch, by the
    /// clock of the device that made it. The newest change wins.
    pub modified_at: i64,
    /// Server-assigned and increasing with every stored change, across all
    /// kinds; used as the `since` cursor for [`ItemChanges`].
    pub revision: i64,
}

/// `GET /items?since=N`: every record whose revision is greater than `N`, in
/// revision order, and the revision to ask from next time.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemChanges {
    pub records: Vec<ItemRecord>,
    pub revision: i64,
}

/// A change made on a device: a created or edited item, or a deletion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemChange {
    pub kind: Kind,
    pub id: Uuid,
    /// `None` deletes the item.
    pub data: Option<serde_json::Value>,
    pub modified_at: i64,
}

/// `POST /items`. The server stores each change unless it already holds a
/// newer one for that kind and id (a larger `modified_at`; ties go to the
/// incoming change).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPushRequest {
    pub changes: Vec<ItemChange>,
}

/// The server's resulting record for every pushed change, in request order. A
/// record differs from what was pushed when the server kept a newer change.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemPushResponse {
    pub records: Vec<ItemRecord>,
}

/// `/hosts`: the server's current state of one host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRecord {
    pub id: Uuid,
    /// `None` once the host has been deleted (a tombstone).
    pub host: Option<Host>,
    pub modified_at: i64,
    pub revision: i64,
}

/// `GET /hosts?since=N`, like [`ItemChanges`] for hosts only.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub records: Vec<HostRecord>,
    pub revision: i64,
}

/// `/hosts`: a host created, edited or deleted on a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostChange {
    pub id: Uuid,
    /// `None` deletes the host. When present, `host.id` must equal `id`.
    pub host: Option<Host>,
    pub modified_at: i64,
}

/// `POST /hosts`, like [`ItemPushRequest`] for hosts only.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    pub changes: Vec<HostChange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushResponse {
    pub records: Vec<HostRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    /// One of the [`error_code`] constants.
    pub error: String,
    pub message: String,
}

pub mod error_code {
    /// 422: malformed email, short password, invalid item, mismatched ids.
    pub const INVALID: &str = "invalid";
    /// 409: sign-up with an email that already has an account.
    pub const EMAIL_TAKEN: &str = "emailTaken";
    /// 401: wrong email or password at log-in.
    pub const INVALID_CREDENTIALS: &str = "invalidCredentials";
    /// 409: a derived login key was sent for an account that still uses its
    /// plain password; log in with [`crate::KEY_VERSION_PLAIN`], then upgrade.
    pub const PLAIN_PASSWORD: &str = "plainPassword";
    /// 401: missing, unknown or revoked session token.
    pub const UNAUTHORIZED: &str = "unauthorized";
    /// 429: too many failed log-ins from the client's address.
    pub const TOO_MANY_ATTEMPTS: &str = "tooManyAttempts";
    /// 423: the account is locked after too many failed log-ins in a row,
    /// even for the right password.
    pub const ACCOUNT_LOCKED: &str = "accountLocked";
    /// 500: anything else.
    pub const INTERNAL: &str = "internal";
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host {
            id: Uuid::nil(),
            label: "db".into(),
            hostname: "db.example.com".into(),
            port: 2222,
            username: "deploy".into(),
            auth: AuthMethod::Key {
                path: "~/.ssh/id_ed25519".into(),
            },
            group: None,
            mosh: false,
        }
    }

    fn forward(kind: ForwardKind) -> Forward {
        Forward {
            id: Uuid::new_v4(),
            label: String::new(),
            host_id: Uuid::new_v4(),
            kind,
            bind_address: "127.0.0.1".into(),
            bind_port: 8080,
            destination_host: Some("localhost".into()),
            destination_port: Some(80),
        }
    }

    #[test]
    fn host_json_is_camel_case_and_omits_defaults() {
        let host = host();
        let json = serde_json::to_value(&host).unwrap();
        assert_eq!(json["auth"]["type"], "key");
        assert!(json.get("group").is_none());
        assert!(json.get("mosh").is_none());

        let record = HostRecord {
            id: host.id,
            host: Some(host),
            modified_at: 1,
            revision: 2,
        };
        let json = serde_json::to_value(&record).unwrap();
        assert_eq!(json["modifiedAt"], 1);
        assert_eq!(json["revision"], 2);
    }

    #[test]
    fn deletion_serializes_as_null() {
        let change = ItemChange {
            kind: Kind::Snippet,
            id: Uuid::nil(),
            data: None,
            modified_at: 5,
        };
        let json = serde_json::to_value(&change).unwrap();
        assert_eq!(json["kind"], "snippet");
        assert!(json["data"].is_null());
    }

    #[test]
    fn validation_reports_the_first_problem() {
        let host: Host =
            serde_json::from_str(r#"{"hostname":"a b","username":"x","auth":{"type":"agent"}}"#)
                .unwrap();
        assert_eq!(
            host.validate().unwrap_err(),
            "hostname cannot contain spaces"
        );
    }

    #[test]
    fn items_round_trip_and_must_match_their_id() {
        let host = host();
        let item = Item::Host(host.clone());
        assert_eq!(
            Item::decode(Kind::Host, host.id, item.encode()).unwrap(),
            item
        );
        let other = Uuid::new_v4();
        assert!(
            Item::decode(Kind::Host, other, item.encode())
                .unwrap_err()
                .contains("does not match")
        );
        // The wrong kind for the data fails to decode.
        assert!(Item::decode(Kind::Secret, host.id, item.encode()).is_err());
    }

    #[test]
    fn forward_rules_need_a_destination_unless_dynamic() {
        assert!(forward(ForwardKind::Local).validate().is_ok());
        let mut remote = forward(ForwardKind::Remote);
        remote.destination_port = None;
        assert!(remote.validate().is_err());
        let mut dynamic = forward(ForwardKind::Dynamic);
        assert!(dynamic.validate().is_err());
        dynamic.destination_host = None;
        dynamic.destination_port = None;
        assert!(dynamic.validate().is_ok());
        dynamic.bind_port = 0;
        assert!(dynamic.validate().is_err());
    }

    #[test]
    fn sealed_secrets_are_checked_for_shape_only() {
        let sealed = format!("v1.{}", "A".repeat(60));
        assert!(validate_sealed(&sealed).is_ok());
        assert!(validate_sealed("hunter2").is_err());
        assert!(validate_sealed("v1.short").is_err());
        assert!(validate_sealed(&format!("v1.{}", "a+b/".repeat(20))).is_err());
    }

    #[test]
    fn plain_credentials_serialize_as_before() {
        let credentials = Credentials {
            email: "me@example.com".into(),
            password: "pw".into(),
            key_version: KEY_VERSION_PLAIN,
        };
        let json = serde_json::to_value(&credentials).unwrap();
        assert!(json.get("keyVersion").is_none());
        let parsed: NewAccount =
            serde_json::from_str(r#"{"email":"a@b.c","password":"x"}"#).unwrap();
        assert_eq!(parsed.credentials.key_version, KEY_VERSION_PLAIN);
        assert_eq!(parsed.vault_key, None);
    }
}
