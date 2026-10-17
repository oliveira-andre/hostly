//! HTTP client for the Hostly sync server API (see `hostly_protocol`).

use std::time::Duration;

use hostly_protocol::{
    API_PREFIX, Account, ApiError, Credentials, ItemChange, ItemChanges, ItemPushRequest,
    ItemPushResponse, KEY_VERSION_DERIVED, KEY_VERSION_PLAIN, NewAccount, PasswordChange, Session,
    error_code,
};
use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;

use super::SyncApi;
use crate::vault::{AccountKeys, VaultKey};
use crate::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ApiClient {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

/// A new session, with the account's vault key unwrapped on this device.
pub struct LoggedIn {
    pub session: Session,
    pub vault: VaultKey,
}

/// Argon2 is deliberately slow, so derivation runs off the async workers.
async fn derive(email: &str, password: &str) -> Result<AccountKeys> {
    let (email, password) = (email.to_owned(), password.to_owned());
    tokio::task::spawn_blocking(move || AccountKeys::derive(&email, &password))
        .await
        .map_err(|e| Error::Vault(e.to_string()))?
}

impl ApiClient {
    /// `server` is the server's base URL, e.g. `https://sync.example.com`.
    pub fn new(server: &str, token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            base: Self::normalize_server(server)?,
            token,
            http,
        })
    }

    /// The same server, authenticated with `token`.
    pub fn with_token(&self, token: String) -> Self {
        Self {
            base: self.base.clone(),
            token: Some(token),
            http: self.http.clone(),
        }
    }

    /// Trims whitespace and trailing slashes; requires an http(s) URL.
    pub fn normalize_server(server: &str) -> Result<String> {
        let server = server.trim().trim_end_matches('/');
        let rest = server
            .strip_prefix("https://")
            .or_else(|| server.strip_prefix("http://"))
            .ok_or(Error::InvalidServerUrl)?;
        if rest.is_empty() || rest.contains(char::is_whitespace) {
            return Err(Error::InvalidServerUrl);
        }
        Ok(server.to_owned())
    }

    /// Checks that the server answers `GET /health` the way a Hostly sync server does.
    pub async fn health(&self) -> Result<()> {
        let response = self
            .http
            .get(format!("{}/health", self.base))
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        let healthy = response.status().is_success()
            && response.text().await.is_ok_and(|body| body.trim() == "ok");
        if healthy {
            Ok(())
        } else {
            Err(Error::NotASyncServer)
        }
    }

    /// Creates an end-to-end encrypted account with a fresh vault key. The
    /// password itself never leaves this device.
    pub async fn create_account(&self, email: &str, password: &str) -> Result<LoggedIn> {
        let keys = derive(email, password).await?;
        let vault = VaultKey::generate()?;
        let request = NewAccount {
            credentials: Credentials {
                email: email.to_owned(),
                password: keys.login_key.clone(),
                key_version: KEY_VERSION_DERIVED,
            },
            vault_key: Some(vault.wrap(&keys)?),
        };
        let session = self.sign_up(&request).await?;
        Ok(LoggedIn { session, vault })
    }

    /// Logs in with the login key derived from `password`. An account created
    /// by an app from before end-to-end encryption logs in with the plain
    /// password once and is upgraded on the spot.
    pub async fn open_account(&self, email: &str, password: &str) -> Result<LoggedIn> {
        let keys = derive(email, password).await?;
        let derived = Credentials {
            email: email.to_owned(),
            password: keys.login_key.clone(),
            key_version: KEY_VERSION_DERIVED,
        };
        match self.log_in(&derived).await {
            Ok(session) => {
                let wrapped = session.vault_key.as_deref().ok_or_else(|| {
                    Error::Vault("the server didn't send the account's vault key".into())
                })?;
                let vault = VaultKey::unwrap(wrapped, &keys)?;
                Ok(LoggedIn { session, vault })
            }
            Err(Error::Api { code, .. }) if code == error_code::PLAIN_PASSWORD => {
                self.upgrade_account(email, password, keys).await
            }
            Err(error) => Err(error),
        }
    }

    async fn upgrade_account(
        &self,
        email: &str,
        password: &str,
        keys: AccountKeys,
    ) -> Result<LoggedIn> {
        let mut session = self
            .log_in(&Credentials {
                email: email.to_owned(),
                password: password.to_owned(),
                key_version: KEY_VERSION_PLAIN,
            })
            .await?;
        let vault = VaultKey::generate()?;
        let wrapped = vault.wrap(&keys)?;
        self.with_token(session.token.clone())
            .put_password(&PasswordChange {
                current_password: password.to_owned(),
                current_key_version: KEY_VERSION_PLAIN,
                new_password: keys.login_key,
                vault_key: wrapped.clone(),
            })
            .await?;
        session.vault_key = Some(wrapped);
        Ok(LoggedIn { session, vault })
    }

    /// Changes the account password, re-wrapping `vault` for the new one.
    /// Saved secrets and other devices' sessions keep working.
    pub async fn change_password(
        &self,
        email: &str,
        current: &str,
        new: &str,
        vault: &VaultKey,
    ) -> Result<()> {
        let current_keys = derive(email, current).await?;
        let new_keys = derive(email, new).await?;
        self.put_password(&PasswordChange {
            current_password: current_keys.login_key,
            current_key_version: KEY_VERSION_DERIVED,
            new_password: new_keys.login_key.clone(),
            vault_key: vault.wrap(&new_keys)?,
        })
        .await
    }

    pub async fn sign_up(&self, request: &NewAccount) -> Result<Session> {
        self.json(self.request(Method::POST, "/accounts").json(request))
            .await
    }

    pub async fn log_in(&self, credentials: &Credentials) -> Result<Session> {
        self.json(self.request(Method::POST, "/sessions").json(credentials))
            .await
    }

    async fn put_password(&self, change: &PasswordChange) -> Result<()> {
        self.send(self.request(Method::PUT, "/account/password").json(change))
            .await
            .map(drop)
    }

    /// Revokes the session token on the server.
    pub async fn log_out(&self) -> Result<()> {
        self.send(self.request(Method::DELETE, "/sessions/current"))
            .await
            .map(drop)
    }

    pub async fn account(&self) -> Result<Account> {
        self.json(self.request(Method::GET, "/account")).await
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self
            .http
            .request(method, format!("{}{API_PREFIX}{path}", self.base));
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn json<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        self.send(request)
            .await?
            .json()
            .await
            .map_err(|e| Error::Network(e.to_string()))
    }

    /// Sends the request; an error status becomes [`Error::Api`].
    async fn send(&self, request: RequestBuilder) -> Result<Response> {
        let response = request
            .send()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let error = response.json::<ApiError>().await.unwrap_or(ApiError {
            error: error_code::INTERNAL.into(),
            message: format!("the sync server answered {status}"),
        });
        Err(Error::Api {
            status: status.as_u16(),
            code: error.error,
            message: error.message,
        })
    }
}

impl SyncApi for ApiClient {
    async fn pull(&self, since: i64) -> Result<ItemChanges> {
        self.json(self.request(Method::GET, &format!("/items?since={since}")))
            .await
    }

    async fn push(&self, changes: Vec<ItemChange>) -> Result<ItemPushResponse> {
        self.json(
            self.request(Method::POST, "/items")
                .json(&ItemPushRequest { changes }),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls_are_normalized() {
        assert_eq!(
            ApiClient::normalize_server(" https://sync.example.com/ ").unwrap(),
            "https://sync.example.com"
        );
        assert_eq!(
            ApiClient::normalize_server("http://localhost:8080").unwrap(),
            "http://localhost:8080"
        );
        for bad in ["sync.example.com", "ftp://x", "https://", "https://a b"] {
            assert!(matches!(
                ApiClient::normalize_server(bad),
                Err(Error::InvalidServerUrl)
            ));
        }
    }
}
