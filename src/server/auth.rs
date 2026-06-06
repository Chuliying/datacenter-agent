//! Bearer-token gate.
//!
//! Every request must carry `Authorization: Bearer <GLOBAL_TOKEN>`.
//! Anything else would be rejected with `418 I'm a teapot` message.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use constant_time_eq::constant_time_eq;
use tracing::debug;

use super::error::ErrorBody;
use super::AppState;

const TEAPOT_MESSAGE: &str = "I'm a teapot. We cannot brew \
coffee for unauthenticated guests. Please present a valid bearer token in the \
Authorization header.";

/// Axum middleware for checking bearer token
///
/// ## Note
///
/// Apply via `middleware::from_fn_with_state(state.clone(), require_bearer)`
pub async fn require_bearer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    // Check if the request has a valid bearer token
    if check(&state, &req) {
        // If so, continue
        return next.run(req).await;
    } else {
        // If not, reject
        debug!(
                path = %req.uri().path(),
                method = %req.method(),
            "auth: rejected with 418"
        );

        // Reject with 418 I'm a teapot and teapot message
        (
            StatusCode::IM_A_TEAPOT,
            Json(ErrorBody::new(TEAPOT_MESSAGE)),
        )
            .into_response()
    }
}

/// Check if the request has a valid bearer token
fn check(state: &AppState, req: &Request) -> bool {
    // Get the authorization header
    let Some(raw) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    else {
        // No authorization header, reject
        return false;
    };

    // `Bearer ` is case-insensitive on the scheme name per RFC 6750.
    let Some(token) = raw.strip_prefix("Bearer ").or_else(|| {
        if raw.len() >= 7 && raw[..6].eq_ignore_ascii_case("Bearer") && raw.as_bytes()[6] == b' ' {
            // Crop "Bearer " (7 characters) from the raw header value
            Some(&raw[7..])
        } else {
            None
        }
    }) else {
        // No bearer token, reject
        return false;
    };

    // Compare the token with the stored token
    constant_time_eq(token.as_bytes(), state.auth_token.as_bytes())
}
