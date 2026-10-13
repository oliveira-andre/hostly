//! The Hostly sync server: accounts, sessions and host sync over the JSON API
//! described in [`hostly_protocol`], stored in PostgreSQL.

mod accounts;
mod error;
mod hosts;

use axum::Router;
use axum::routing::{delete, get, post};
use hostly_protocol::API_PREFIX;
use sqlx::PgPool;
use tower_http::trace::TraceLayer;

/// Applies the database migrations embedded from `migrations/`.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}

pub fn app(pool: PgPool) -> Router {
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
        .with_state(pool)
}
