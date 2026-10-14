//! Types shared by the Hostly app and the Hostly sync server: the host model and
//! the JSON API. All JSON is camelCase.
//!
//! # API (`/api/v1`)
//!
//! | Method and path         | Body           | Success                 | Auth   |
//! |-------------------------|----------------|-------------------------|--------|
//! | `POST /accounts`        | [`Credentials`] | 201 [`Session`]        | –      |
//! | `POST /sessions`        | [`Credentials`] | 200 [`Session`]        | –      |
//! | `DELETE /sessions/current` | –           | 204                     | Bearer |
//! | `GET /account`          | –              | 200 [`Account`]         | Bearer |
//! | `GET /hosts?since=N`    | –              | 200 [`Changes`]         | Bearer |
//! | `POST /hosts`           | [`PushRequest`] | 200 [`PushResponse`]   | Bearer |
//!
//! `GET /health` (outside `/api/v1`) answers 200 `ok`. Failures answer with an
//! [`ApiError`] body; see [`error_code`] for the codes.
//!
//! `POST /sessions` is limited: too many failed log-ins from one address, or
//! in a row for one account, are refused for a while (429
//! [`error_code::TOO_MANY_ATTEMPTS`], 423 [`error_code::ACCOUNT_LOCKED`]). Both
//! carry a `Retry-After` header in seconds.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const API_PREFIX: &str = "/api/v1";

/// Minimum account password length, enforced by the server.
pub const MIN_PASSWORD_LEN: usize = 8;

/// A saved connection. Secrets (passwords, key passphrases) are never part of
/// it: they stay in each device's keychain and are not synced.
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

impl Host {
    /// Returns the reason the host can't be saved, if any.
    pub fn validate(&self) -> Result<(), String> {
        let invalid = |reason: &str| Err(reason.to_owned());
        if self.hostname.trim().is_empty() {
            return invalid("hostname is required");
        }
        if self.hostname.chars().any(char::is_whitespace) {
            return invalid("hostname cannot contain spaces");
        }
        if self.username.trim().is_empty() {
            return invalid("username is required");
        }
        if self.port == 0 {
            return invalid("port must be between 1 and 65535");
        }
        if let AuthMethod::Key { path } = &self.auth
            && path.as_os_str().is_empty()
        {
            return invalid("private key path is required");
        }
        Ok(())
    }
}

/// Sign-up and log-in request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credentials {
    pub email: String,
    pub password: String,
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
}

/// The server's current state of one host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostRecord {
    pub id: Uuid,
    /// `None` once the host has been deleted (a tombstone).
    pub host: Option<Host>,
    /// When the change was made, in milliseconds since the Unix epoch, by the
    /// clock of the device that made it. The newest change wins.
    pub modified_at: i64,
    /// Server-assigned and increasing with every stored change; used as the
    /// `since` cursor for [`Changes`].
    pub revision: i64,
}

/// `GET /hosts?since=N`: every record whose revision is greater than `N`, in
/// revision order, and the revision to ask from next time.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    pub records: Vec<HostRecord>,
    pub revision: i64,
}

/// A change made on a device: a created or edited host, or a deletion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostChange {
    pub id: Uuid,
    /// `None` deletes the host. When present, `host.id` must equal `id`.
    pub host: Option<Host>,
    pub modified_at: i64,
}

/// `POST /hosts`. The server stores each change unless it already holds a
/// newer one for that id (a larger `modified_at`; ties go to the incoming change).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PushRequest {
    pub changes: Vec<HostChange>,
}

/// The server's resulting record for every pushed id, in request order. A
/// record differs from what was pushed when the server kept a newer change.
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
    /// 422: malformed email, short password, invalid host, mismatched ids.
    pub const INVALID: &str = "invalid";
    /// 409: sign-up with an email that already has an account.
    pub const EMAIL_TAKEN: &str = "emailTaken";
    /// 401: wrong email or password at log-in.
    pub const INVALID_CREDENTIALS: &str = "invalidCredentials";
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

    #[test]
    fn host_json_is_camel_case_and_omits_empty_group() {
        let host = Host {
            id: Uuid::nil(),
            label: "db".into(),
            hostname: "db.example.com".into(),
            port: 2222,
            username: "deploy".into(),
            auth: AuthMethod::Key {
                path: "~/.ssh/id_ed25519".into(),
            },
            group: None,
        };
        let json = serde_json::to_value(&host).unwrap();
        assert_eq!(json["auth"]["type"], "key");
        assert!(json.get("group").is_none());

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
    fn deletion_serializes_host_as_null() {
        let change = HostChange {
            id: Uuid::nil(),
            host: None,
            modified_at: 5,
        };
        let json = serde_json::to_value(&change).unwrap();
        assert!(json["host"].is_null());
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
}
