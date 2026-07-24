// Copyright 2026 Wayne Hong (h-alice) <contact@halice.art>
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
use super::openai::{OpenAiErrorBody, ERR_INVALID_REQUEST};
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
        next.run(req).await
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

/// OpenAI-envelope bearer gate for `POST /v1/chat/completions` (finding #6).
///
/// The token check is identical to [`require_bearer`] (same constant-time compare against
/// `GLOBAL_TOKEN`), but a rejection is `401 Unauthorized` carrying the OpenAI error envelope —
/// what an OpenAI client / agentgateway expects — instead of the host's `418` teapot. The shared
/// [`require_bearer`] middleware (and its D6 `418` contract for the other seven endpoints) is
/// deliberately left untouched.
///
/// ## Note
///
/// Apply via `middleware::from_fn_with_state(state.clone(), require_bearer_openai)`.
pub async fn require_bearer_openai(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if check(&state, &req) {
        next.run(req).await
    } else {
        debug!(
            path = %req.uri().path(),
            method = %req.method(),
            "auth: rejected /v1 with 401"
        );
        openai_unauthorized()
    }
}

/// The `401` + OpenAI error envelope returned when the `/v1/chat/completions` bearer check fails.
fn openai_unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(OpenAiErrorBody::new(
            ERR_INVALID_REQUEST,
            "missing or invalid bearer token",
        )),
    )
        .into_response()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn openai_unauthorized_is_401_with_openai_envelope() {
        // Finding #6: the /v1 bearer gate rejects with 401 + OpenAI error envelope (not 418).
        let resp = openai_unauthorized();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(v["error"]["type"], "invalid_request_error");
        assert!(
            v["error"]["message"].is_string(),
            "envelope must carry a message"
        );
    }
}
