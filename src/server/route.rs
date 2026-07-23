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

//! axum router assembly + middleware stack.

use std::time::Duration;

use axum::error_handling::HandleErrorLayer;
use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{BoxError, Json, Router};
use tower::timeout::TimeoutLayer as TowerTimeoutLayer;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use super::auth::{require_bearer, require_bearer_openai};
use super::handler;
use super::{openai, AppState};

/// Convert a middleware error on the OpenAI sub-router into the OpenAI **error envelope**
/// (`{"error":{"message","type"}}`), so `/v1/chat/completions` never returns the empty body a bare
/// [`TimeoutLayer`] would (finding #4). The only middleware error today is a request timeout from
/// the [`tower::timeout`] layer → `504`; any other error maps to `500`. Both are `server_error`, the
/// same `type` the handler uses for its own 5xx (see [`openai::error_type_for_status`]).
async fn handle_openai_middleware_error(err: BoxError) -> Response {
    let (status, message) = if err.is::<tower::timeout::error::Elapsed>() {
        (
            StatusCode::GATEWAY_TIMEOUT,
            format!(
                "request timed out after {}s",
                OPENAI_REQUEST_TIMEOUT.as_secs()
            ),
        )
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("internal error: {err}"),
        )
    };
    (
        status,
        Json(openai::OpenAiErrorBody::new(
            openai::error_type_for_status(status.as_u16()),
            message,
        )),
    )
        .into_response()
}

/// 64 KiB. Defense-in-depth above the per-field 2 000-char prompt cap.
const REQUEST_BODY_LIMIT: usize = 64 * 1024;

/// 120 s. The slow path is the LLM round-trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// 600 s. The OpenAI-compatible `/v1/chat/completions` non-streaming path awaits the **entire**
/// multi-stage sub-agent pipeline before it can return a `chat.completion`, which routinely runs
/// past the standard 120 s and would otherwise be cut off as an empty `504` (finding #1). Streaming
/// responses return their SSE handle promptly, so the request timeout never bites them either way.
const OPENAI_REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

pub fn build_router(state: AppState) -> Router {
    // Cross-cutting middleware shared by every route. The per-request timeout is applied per group
    // below (the standard endpoints keep 120 s; `/v1/chat/completions` gets a longer ceiling), so
    // it is intentionally *not* part of this shared stack.
    let shared = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::very_permissive())
        .layer(CompressionLayer::new())
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ));

    // The seven original endpoints: standard 120 s timeout, `require_bearer` (D6, `418` on a bad
    // token). Auth + timeout are applied to this sub-router so they stay scoped to these routes.
    let standard = Router::new()
        .route("/health", get(handler::health))
        .route("/ready", get(handler::ready))
        .route("/greeting", get(handler::greeting))
        .route("/insight", post(handler::insight))
        .route("/insight/stream", post(handler::insight_stream))
        .route("/report", post(handler::report))
        .route("/report/stream", post(handler::report_stream))
        .route("/agent/stream", post(handler::agent_stream))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), require_bearer));

    // OpenAI-compatible endpoint (agentgateway Path C): longer timeout for the full-pipeline
    // non-streaming path (finding #1), and a dedicated bearer gate that rejects with `401` + the
    // OpenAI error envelope rather than `418` (finding #6). The shared `require_bearer` (and its
    // D6 contract for the seven endpoints above) is left untouched.
    let openai = Router::new()
        .route("/v1/chat/completions", post(handler::chat_completions))
        // `HandleErrorLayer` (outer) catches the `tower::timeout` layer's `Elapsed` error and turns
        // it into the OpenAI error envelope, instead of the empty body `tower_http`'s
        // `with_status_code` variant would send (finding #4). Auth stays outermost (applied last).
        .layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_openai_middleware_error))
                .layer(TowerTimeoutLayer::new(OPENAI_REQUEST_TIMEOUT)),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer_openai,
        ));

    // The body limit + shared middleware wrap both groups; each group already carries its own
    // timeout and auth from above.
    Router::new()
        .merge(standard)
        .merge(openai)
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT))
        .layer(shared)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `oneshot`

    /// A handler slow enough to trip a short timeout but finish under a long one.
    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_millis(300)).await;
        "ok"
    }

    /// Finding #1: merging two sub-routers that each carry their own `TimeoutLayer` keeps the
    /// timeouts scoped per group — a slow handler is cut off (`504`) on the short-timeout group but
    /// survives (`200`) on the long-timeout group. This is exactly the structure `build_router`
    /// uses for the standard endpoints (120 s) vs `/v1/chat/completions` (600 s); the durations are
    /// shrunk here so the test is fast and its margins are wide enough to be non-flaky.
    #[tokio::test]
    async fn per_group_timeout_layers_survive_a_merge() {
        let short = Router::new().route("/standard", get(slow)).layer(
            TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_millis(50)),
        );
        let long = Router::new().route("/openai", get(slow)).layer(
            TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, Duration::from_secs(3)),
        );
        let app = Router::new().merge(short).merge(long);

        // Short-timeout group: the 300 ms handler is cut off with a 504.
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/standard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);

        // Long-timeout group: the same handler finishes with a 200.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/openai")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Finding #4: a timeout on the OpenAI sub-router must return the OpenAI **error envelope**
    /// (`{"error":{"type":"server_error",...}}`), not the empty body a bare `TimeoutLayer` sends.
    #[tokio::test]
    async fn openai_timeout_returns_openai_error_envelope() {
        let app = Router::new().route("/v1/chat/completions", post(slow)).layer(
            ServiceBuilder::new()
                .layer(HandleErrorLayer::new(handle_openai_middleware_error))
                .layer(TowerTimeoutLayer::new(Duration::from_millis(50))),
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes)
            .expect("timeout body must be JSON (the OpenAI error envelope)");
        assert_eq!(v["error"]["type"], "server_error");
        assert!(
            v["error"]["message"].is_string(),
            "envelope must carry a message string, got {v}"
        );
    }
}
