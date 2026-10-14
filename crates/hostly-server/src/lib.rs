//! The Hostly sync server: accounts, sessions and host sync over the JSON API
//! described in [`hostly_protocol`], stored in PostgreSQL.

mod accounts;
mod error;
mod hosts;
mod login_limits;

use std::sync::Arc;

use axum::Router;
use axum::extract::FromRef;
use axum::routing::{delete, get, post};
use hostly_protocol::API_PREFIX;
use login_limits::LoginGuard;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

pub use login_limits::LoginLimits;

/// Applies the database migrations embedded from `migrations/`.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

#[derive(Clone, FromRef)]
struct AppState {
    pool: PgPool,
    logins: Arc<LoginGuard>,
}

/// The server's routes. Serve them with
/// `into_make_service_with_connect_info::<SocketAddr>()`: log-in limits need
/// each client's address.
pub fn app(pool: PgPool, limits: LoginLimits) -> Router {
    let api = Router::new()
        .route("/accounts", post(accounts::sign_up))
        .route("/sessions", post(accounts::log_in))
        .route("/sessions/current", delete(accounts::log_out))
        .route("/account", get(accounts::account))
        .route("/hosts", get(hosts::pull).post(hosts::push));
    Router::new()
        .nest(API_PREFIX, api)
        .route("/health", get(|| async { "ok" }))
        .layer(TraceLayer::new_for_http())
        .with_state(AppState {
            pool,
            logins: Arc::new(LoginGuard::new(limits)),
        })
}
