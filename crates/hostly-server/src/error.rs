use std::fmt::Display;
use std::time::Duration;

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::StatusCode;
use axum::http::header::RETRY_AFTER;
use axum::response::{IntoResponse, Response};
use hostly_protocol::{ApiError, error_code};

/// A failure answered with an [`ApiError`] body.
#[derive(Debug)]
pub struct ApiFailure {
    status: StatusCode,
    code: &'static str,
    message: String,
    retry_after: Option<Duration>,
}

impl ApiFailure {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    /// Adds a `Retry-After` header, in whole seconds rounded up.
    pub fn retry_after(mut self, wait: Duration) -> Self {
        self.retry_after = Some(wait);
        self
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            error_code::INVALID,
            message,
        )
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            error_code::UNAUTHORIZED,
            "missing, unknown or revoked session token",
        )
    }

    /// Logs the cause; the client only learns that something went wrong.
    pub fn internal(cause: impl Display) -> Self {
        tracing::error!(%cause, "internal error");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            error_code::INTERNAL,
            "internal server error",
        )
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let body = ApiError {
            error: self.code.to_owned(),
            message: self.message,
        };
        let mut response = (self.status, Json(body)).into_response();
        if let Some(wait) = self.retry_after {
            response
                .headers_mut()
                .insert(RETRY_AFTER, whole_seconds(wait).into());
        }
        response
    }
}

/// Rounds up, so a client that waits this long is never early.
pub fn whole_seconds(duration: Duration) -> u64 {
    duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}

impl From<sqlx::Error> for ApiFailure {
    fn from(error: sqlx::Error) -> Self {
        Self::internal(error)
    }
}

impl From<JsonRejection> for ApiFailure {
    fn from(rejection: JsonRejection) -> Self {
        Self::invalid(rejection.body_text())
    }
}

impl From<QueryRejection> for ApiFailure {
    fn from(rejection: QueryRejection) -> Self {
        Self::invalid(rejection.body_text())
    }
}

/// `axum::Json` whose rejections are [`ApiFailure`]s.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiFailure))]
pub struct ApiJson<T>(pub T);

/// `axum::extract::Query` whose rejections are [`ApiFailure`]s.
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiFailure))]
pub struct ApiQuery<T>(pub T);
