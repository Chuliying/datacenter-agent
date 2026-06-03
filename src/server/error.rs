//! Error types

use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// Uniform error envelope returned by every non-2xx response.
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub error: String,
}

impl ErrorBody {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { error: msg.into() }
    }
}

// ──── HTTP errors ────

#[derive(Debug)]
pub enum AppError {
    /// Client-side problem: malformed JSON, missing field, validation failure.
    BadRequest(String),
    /// LLM upstream failed.
    BadGateway(String),
    /// Service is not ready (e.g. missing config, upstream unreachable).
    ServiceUnavailable(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::BadGateway(_) => StatusCode::BAD_GATEWAY,
            AppError::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn message(&self) -> &str {
        match self {
            AppError::BadRequest(m) | AppError::BadGateway(m) | AppError::ServiceUnavailable(m) => {
                m
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = Json(ErrorBody::new(self.message().to_owned()));
        (status, body).into_response()
    }
}

impl From<JsonRejection> for AppError {
    fn from(rej: JsonRejection) -> Self {
        AppError::BadRequest(format!("invalid json: {rej}"))
    }
}
