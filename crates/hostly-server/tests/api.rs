//! HTTP tests against the real server and a real PostgreSQL database.
//!
//! Set `TEST_DATABASE_URL` (or put it in `.env`) to run them; without it they
//! pass after printing a skip notice. Every test signs up its own accounts, so
//! tests can share one database and run in parallel.

use std::cell::Cell;
use std::net::SocketAddr;
use std::time::Duration;

use hostly_protocol::{
    API_PREFIX, Account, ApiError, AuthMethod, Changes, Credentials, Host, HostChange, PushRequest,
    PushResponse, Session, error_code,
};
use hostly_server::LoginLimits;
use reqwest::StatusCode;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use uuid::Uuid;

const PASSWORD: &str = "correct horse battery";

struct TestApi {
    base: String,
    http: reqwest::Client,
}

/// Serves the app on a random port, or returns from the test when no database is configured.
macro_rules! api {
    () => {
        api!(LoginLimits::default())
    };
    ($limits:expr) => {
        match TestApi::start($limits).await {
            Some(api) => api,
            None => return,
        }
    };
}

impl TestApi {
    async fn start(limits: LoginLimits) -> Option<Self> {
        let _ = dotenvy::dotenv();
        let Some(url) = std::env::var("TEST_DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty())
        else {
            eprintln!("skipping: TEST_DATABASE_URL is not set");
            return None;
        };
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await
            .expect("connect to TEST_DATABASE_URL");
        hostly_server::migrate(&pool).await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let app = hostly_server::app(pool, limits);
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        });
        Some(Self {
            base,
            http: reqwest::Client::new(),
        })
    }

    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let url = if path == "/health" {
            format!("{}{path}", self.base)
        } else {
            format!("{}{API_PREFIX}{path}", self.base)
        };
        let request = self.http.request(method, url);
        match token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    async fn post(
        &self,
        path: &str,
        token: Option<&str>,
        body: &impl Serialize,
    ) -> reqwest::Response {
        self.request(reqwest::Method::POST, path, token)
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn get(&self, path: &str, token: Option<&str>) -> reqwest::Response {
        self.request(reqwest::Method::GET, path, token)
            .send()
            .await
            .unwrap()
    }

    async fn sign_up(&self, email: &str) -> Session {
        let response = self
            .post("/accounts", None, &credentials(email, PASSWORD))
            .await;
        assert_eq!(response.status(), StatusCode::CREATED);
        response.json().await.unwrap()
    }

    async fn new_account(&self) -> Session {
        self.sign_up(&unique_email()).await
    }

    /// Logs in as if through a reverse proxy, from `client`.
    async fn log_in_from(&self, client: &str, email: &str, password: &str) -> reqwest::Response {
        self.request(reqwest::Method::POST, "/sessions", None)
            .header("x-forwarded-for", client)
            .json(&credentials(email, password))
            .send()
            .await
            .unwrap()
    }

    async fn push(&self, token: &str, changes: Vec<HostChange>) -> PushResponse {
        let response = self
            .post("/hosts", Some(token), &PushRequest { changes })
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }

    async fn pull(&self, token: &str, since: i64) -> Changes {
        let response = self
            .get(&format!("/hosts?since={since}"), Some(token))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        response.json().await.unwrap()
    }
}

/// Asserts the status and error code, and returns the message.
async fn expect_error(response: reqwest::Response, status: StatusCode, code: &str) -> String {
    assert_eq!(response.status(), status);
    let error: ApiError = response.json().await.unwrap();
    assert_eq!(error.error, code);
    error.message
}

fn retry_after(response: &reqwest::Response) -> u64 {
    response.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap()
}

fn unique_email() -> String {
    format!("user-{}@example.com", Uuid::new_v4())
}

fn credentials(email: &str, password: &str) -> Credentials {
    Credentials {
        email: email.into(),
        password: password.into(),
    }
}

fn host(id: Uuid, label: &str) -> Host {
    Host {
        id,
        label: label.into(),
        hostname: "storage.example.com".into(),
        port: 22,
        username: "deploy".into(),
        auth: AuthMethod::Password,
        group: None,
    }
}

fn save(host: Host, modified_at: i64) -> HostChange {
    HostChange {
        id: host.id,
        host: Some(host),
        modified_at,
    }
}

fn delete(id: Uuid, modified_at: i64) -> HostChange {
    HostChange {
        id,
        host: None,
        modified_at,
    }
}

#[tokio::test]
async fn health_is_ok() {
    let api = api!();
    let response = api.get("/health", None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn sign_up_log_in_account_and_log_out() {
    let api = api!();
    let email = unique_email();
    let first = api.sign_up(&format!("  {}  ", email.to_uppercase())).await;
    assert_eq!(first.account.email, email, "stored trimmed and lowercased");
    assert!(!first.token.is_empty());

    let response = api.get("/account", Some(&first.token)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Account>().await.unwrap(), first.account);

    let response = api
        .post("/sessions", None, &credentials(&email, PASSWORD))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let second: Session = response.json().await.unwrap();
    assert_eq!(second.account, first.account);
    assert_ne!(second.token, first.token);

    let response = api
        .request(
            reqwest::Method::DELETE,
            "/sessions/current",
            Some(&second.token),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let response = api.get("/account", Some(&second.token)).await;
    expect_error(response, StatusCode::UNAUTHORIZED, error_code::UNAUTHORIZED).await;
    // Logging out revokes only that session.
    let response = api.get("/account", Some(&first.token)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn duplicate_email_is_rejected_regardless_of_case() {
    let api = api!();
    let email = unique_email();
    api.sign_up(&email).await;
    let response = api
        .post(
            "/accounts",
            None,
            &credentials(&email.to_uppercase(), PASSWORD),
        )
        .await;
    expect_error(response, StatusCode::CONFLICT, error_code::EMAIL_TAKEN).await;
}

#[tokio::test]
async fn wrong_password_or_unknown_email_is_invalid_credentials() {
    let api = api!();
    let session = api.new_account().await;
    for attempt in [
        credentials(&session.account.email, "not the password"),
        credentials(&unique_email(), PASSWORD),
    ] {
        let response = api.post("/sessions", None, &attempt).await;
        expect_error(
            response,
            StatusCode::UNAUTHORIZED,
            error_code::INVALID_CREDENTIALS,
        )
        .await;
    }
}

#[tokio::test]
async fn failed_log_ins_are_limited_per_client_address() {
    let api = api!(LoginLimits {
        address_failures: 3,
        account_failures: 100,
        ..LoginLimits::default()
    });
    let email = api.new_account().await.account.email;

    // Successful log-ins don't count.
    for _ in 0..4 {
        let response = api.log_in_from("203.0.113.1", &email, PASSWORD).await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    for _ in 0..3 {
        let response = api
            .log_in_from("203.0.113.1", &unique_email(), PASSWORD)
            .await;
        expect_error(
            response,
            StatusCode::UNAUTHORIZED,
            error_code::INVALID_CREDENTIALS,
        )
        .await;
    }

    // Now that address is refused, even with the right password...
    let response = api.log_in_from("203.0.113.1", &email, PASSWORD).await;
    let wait = retry_after(&response);
    assert!((14 * 60..=15 * 60).contains(&wait), "Retry-After {wait}");
    let message = expect_error(
        response,
        StatusCode::TOO_MANY_REQUESTS,
        error_code::TOO_MANY_ATTEMPTS,
    )
    .await;
    assert!(message.contains("15 minutes"), "{message}");

    // ...while other addresses can still log in.
    let response = api.log_in_from("203.0.113.2", &email, PASSWORD).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn failed_log_ins_in_a_row_lock_the_account_for_longer_each_time() {
    let api = api!(LoginLimits {
        account_failures: 3,
        account_lock: Duration::from_secs(1),
        ..LoginLimits::default()
    });
    let email = api.new_account().await.account.email;
    let bystander = api.new_account().await.account.email;
    // Each attempt comes from a new address, so only the account limit applies.
    let attempts = Cell::new(0);
    let log_in = async |password: &str| {
        attempts.set(attempts.get() + 1);
        let address = format!("198.51.100.{}", attempts.get());
        api.log_in_from(&address, &email, password).await
    };

    // A successful log-in resets the count.
    for _ in 0..2 {
        assert_eq!(log_in("wrong").await.status(), StatusCode::UNAUTHORIZED);
    }
    assert_eq!(log_in(PASSWORD).await.status(), StatusCode::OK);

    let expect_lock_after_failures = async |lock_seconds: u64| {
        for _ in 0..3 {
            let response = log_in("wrong").await;
            expect_error(
                response,
                StatusCode::UNAUTHORIZED,
                error_code::INVALID_CREDENTIALS,
            )
            .await;
        }
        let response = log_in(PASSWORD).await;
        assert_eq!(retry_after(&response), lock_seconds);
        expect_error(response, StatusCode::LOCKED, error_code::ACCOUNT_LOCKED).await;
        tokio::time::sleep(Duration::from_secs(lock_seconds) + Duration::from_millis(200)).await;
    };
    expect_lock_after_failures(1).await;
    // Failing again right after a lock locks it twice as long.
    expect_lock_after_failures(2).await;

    let response = api
        .log_in_from("198.51.100.250", &bystander, PASSWORD)
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "other accounts are unaffected"
    );
    assert_eq!(log_in(PASSWORD).await.status(), StatusCode::OK);
    // After a successful log-in, the next lock is back to the first length.
    expect_lock_after_failures(1).await;
}

#[tokio::test]
async fn requests_need_a_valid_token() {
    let api = api!();
    for token in [None, Some("not-a-real-token")] {
        for response in [
            api.get("/account", token).await,
            api.get("/hosts", token).await,
            api.post("/hosts", token, &PushRequest { changes: vec![] })
                .await,
        ] {
            expect_error(response, StatusCode::UNAUTHORIZED, error_code::UNAUTHORIZED).await;
        }
    }
}

#[tokio::test]
async fn sign_up_input_is_validated() {
    let api = api!();
    let response = api
        .post("/accounts", None, &credentials(&unique_email(), "short"))
        .await;
    let message = expect_error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        error_code::INVALID,
    )
    .await;
    assert!(message.contains("at least"), "{message}");

    for email in ["", "no-at-sign", "@example.com", "two words@example.com"] {
        let response = api
            .post("/accounts", None, &credentials(email, PASSWORD))
            .await;
        expect_error(
            response,
            StatusCode::UNPROCESSABLE_ENTITY,
            error_code::INVALID,
        )
        .await;
    }

    let response = api
        .request(reqwest::Method::POST, "/accounts", None)
        .header("content-type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    expect_error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        error_code::INVALID,
    )
    .await;
}

#[tokio::test]
async fn push_then_pull_follows_the_revision_cursor() {
    let api = api!();
    let token = api.new_account().await.token;

    let empty = api.pull(&token, 0).await;
    assert!(empty.records.is_empty());
    assert_eq!(empty.revision, 0);

    let (a, b) = (host(Uuid::new_v4(), "a"), host(Uuid::new_v4(), "b"));
    let pushed = api
        .push(&token, vec![save(a.clone(), 1), save(b.clone(), 1)])
        .await
        .records;
    assert_eq!(pushed.len(), 2);
    assert_eq!(
        pushed[0].host.as_ref(),
        Some(&a),
        "records follow request order"
    );
    assert_eq!(pushed[1].host.as_ref(), Some(&b));
    assert!(pushed[0].revision < pushed[1].revision);

    let all = api.pull(&token, 0).await;
    assert_eq!(all.records, pushed);
    assert_eq!(all.revision, pushed[1].revision);

    let none = api.pull(&token, all.revision).await;
    assert!(none.records.is_empty());
    assert_eq!(none.revision, all.revision);

    let edited = host(a.id, "a, renamed");
    let record = api
        .push(&token, vec![save(edited.clone(), 2)])
        .await
        .records[0]
        .clone();
    assert!(record.revision > all.revision);
    let after = api.pull(&token, all.revision).await;
    assert_eq!(after.records, vec![record.clone()]);
    assert_eq!(after.revision, record.revision);

    // Without `since`, everything comes back.
    let response = api.get("/hosts", Some(&token)).await;
    let everything: Changes = response.json().await.unwrap();
    assert_eq!(everything.records.len(), 2);
    assert_eq!(everything.records[1].host.as_ref(), Some(&edited));
}

#[tokio::test]
async fn the_newest_change_wins_and_ties_go_to_the_incoming_change() {
    let api = api!();
    let token = api.new_account().await.token;
    let id = Uuid::new_v4();

    let stored = api
        .push(&token, vec![save(host(id, "newer"), 100)])
        .await
        .records[0]
        .clone();
    let stale = api
        .push(&token, vec![save(host(id, "older"), 50)])
        .await
        .records[0]
        .clone();
    assert_eq!(
        stale, stored,
        "an older change is ignored and the stored record returned"
    );
    assert!(api.pull(&token, stored.revision).await.records.is_empty());

    let tie = api
        .push(&token, vec![save(host(id, "same time"), 100)])
        .await
        .records[0]
        .clone();
    assert_eq!(tie.host.unwrap().label, "same time");
    assert!(tie.revision > stored.revision);
}

#[tokio::test]
async fn deletions_are_kept_as_tombstones() {
    let api = api!();
    let token = api.new_account().await.token;
    let a = host(Uuid::new_v4(), "a");

    api.push(&token, vec![save(a.clone(), 1)]).await;
    let deleted = api.push(&token, vec![delete(a.id, 2)]).await.records[0].clone();
    assert_eq!(deleted.host, None);
    assert_eq!(deleted.modified_at, 2);

    let changes = api.pull(&token, 0).await;
    assert_eq!(changes.records, vec![deleted.clone()]);

    // An older edit doesn't resurrect it; a newer one does.
    let still_deleted = api.push(&token, vec![save(a.clone(), 1)]).await.records[0].clone();
    assert_eq!(still_deleted, deleted);
    let restored = api.push(&token, vec![save(a.clone(), 3)]).await.records[0].clone();
    assert_eq!(restored.host, Some(a));
}

#[tokio::test]
async fn invalid_changes_store_nothing() {
    let api = api!();
    let token = api.new_account().await.token;

    let valid = host(Uuid::new_v4(), "valid");
    let mut invalid = host(Uuid::new_v4(), "invalid");
    invalid.hostname = "two words".into();
    let response = api
        .post(
            "/hosts",
            Some(&token),
            &PushRequest {
                changes: vec![save(valid.clone(), 1), save(invalid, 1)],
            },
        )
        .await;
    let message = expect_error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        error_code::INVALID,
    )
    .await;
    assert!(message.contains("spaces"), "{message}");

    let mismatched = HostChange {
        id: Uuid::new_v4(),
        host: Some(valid),
        modified_at: 1,
    };
    let response = api
        .post(
            "/hosts",
            Some(&token),
            &PushRequest {
                changes: vec![mismatched],
            },
        )
        .await;
    expect_error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        error_code::INVALID,
    )
    .await;

    let response = api.get("/hosts?since=abc", Some(&token)).await;
    expect_error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        error_code::INVALID,
    )
    .await;

    assert!(api.pull(&token, 0).await.records.is_empty());
}

#[tokio::test]
async fn accounts_are_isolated_even_with_the_same_host_id() {
    let api = api!();
    let x = api.new_account().await.token;
    let y = api.new_account().await.token;
    let id = Uuid::new_v4();

    api.push(&x, vec![save(host(id, "in x"), 1)]).await;
    api.push(&y, vec![save(host(id, "in y"), 1)]).await;
    api.push(&x, vec![delete(id, 2)]).await;

    let from_x = api.pull(&x, 0).await.records;
    let from_y = api.pull(&y, 0).await.records;
    assert_eq!(from_x.len(), 1);
    assert_eq!(from_x[0].host, None);
    assert_eq!(from_y.len(), 1);
    assert_eq!(from_y[0].host.as_ref().unwrap().label, "in y");
}

#[tokio::test]
async fn concurrent_pushes_get_distinct_increasing_revisions() {
    let api = api!();
    let token = api.new_account().await.token;
    let api = std::sync::Arc::new(api);

    let pushes = (0..10).map(|n| {
        let (api, token) = (api.clone(), token.clone());
        tokio::spawn(async move {
            api.push(
                &token,
                vec![save(host(Uuid::new_v4(), &format!("h{n}")), 1)],
            )
            .await
        })
    });
    for push in pushes {
        push.await.unwrap();
    }

    let changes = api.pull(&token, 0).await;
    assert_eq!(changes.records.len(), 10);
    assert!(
        changes
            .records
            .windows(2)
            .all(|w| w[0].revision < w[1].revision)
    );
    assert_eq!(changes.revision, changes.records[9].revision);
}
