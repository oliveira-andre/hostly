use std::fmt::Display;

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use hostly_protocol::{ApiError, error_code};

/// A failure answered with an [`ApiError`] body.
#[derive(Debug)]
pub struct ApiFailure {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiFailure {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
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
        (self.status, Json(body)).into_response()
    }
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
