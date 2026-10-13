//! HTTP client for the Hostly sync server API (see `hostly_protocol`).

use std::time::Duration;

use hostly_protocol::{
    API_PREFIX, Account, ApiError, Changes, Credentials, HostChange, PushRequest, PushResponse,
    Session, error_code,
};
use reqwest::{Method, RequestBuilder, Response};
use serde::de::DeserializeOwned;

use super::SyncApi;
use crate::{Error, Result};

const TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct ApiClient {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
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

    pub async fn sign_up(&self, credentials: &Credentials) -> Result<Session> {
        self.json(self.request(Method::POST, "/accounts").json(credentials))
            .await
    }

    pub async fn log_in(&self, credentials: &Credentials) -> Result<Session> {
        self.json(self.request(Method::POST, "/sessions").json(credentials))
            .await
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
    async fn pull(&self, since: i64) -> Result<Changes> {
        self.json(self.request(Method::GET, &format!("/hosts?since={since}")))
            .await
    }

    async fn push(&self, changes: Vec<HostChange>) -> Result<PushResponse> {
        self.json(
            self.request(Method::POST, "/hosts")
                .json(&PushRequest { changes }),
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
