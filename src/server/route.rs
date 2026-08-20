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

use std::sync::Arc;

use super::auth::{require_bearer, require_bearer_openai};
use super::handler;
use super::rate_limit::{self, ErrorFamily, GlobalBurstLimiter};
use super::{openai, AppState};
use crate::runtime::audit::{AuditSink, TracingAuditSink};

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

/// 64 KiB. Defense-in-depth above the runtime prelude's per-field prompt cap
/// (`thresholds.input.max_prompt_chars`).
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

    // Opt-in global burst limiter (S-RUNTIME-SEC-01 FR-005): one shared bucket
    // for the expensive routes only. Rejections audit through the runtime's
    // sink when the runtime is wired, else the tracing sink — same stream.
    let limiter = state.rate_limit.enabled.then(|| {
        let audit: Arc<dyn AuditSink> = state
            .runtime
            .as_ref()
            .map(|runtime| runtime.audit_sink.clone())
            .unwrap_or_else(|| Arc::new(TracingAuditSink));
        Arc::new(GlobalBurstLimiter::new(&state.rate_limit, audit))
    });

    // The four standard endpoints: standard 120 s timeout, `require_bearer` (D6, `418` on a bad
    // token). Auth + timeout are applied to this sub-router so they stay scoped to these routes.
    //
    // `/insight`, `/insight/stream`, `/report` and `/report/stream` were retired: `/agent/stream`
    // reaches both sub-agent pipelines through intent routing (see `wants_report_pipeline`), so the
    // forced-pipeline variants were redundant — and they were the only prompt entry points that
    // bypassed the runtime prelude (no guardrails, no audit). Callers use `/agent/stream`
    // (streaming) or `/v1/chat/completions` (non-streaming).
    //
    // `/agent/stream` sits on its own nested router so the opt-in limiter can wrap it without
    // touching `/health`, `/ready`, or `/greeting` (AC-009). Auth is layered after (= outside)
    // the limiter, so a bad token is rejected before any admission capacity is consumed (AC-014).
    let mut agent_stream = Router::new().route("/agent/stream", post(handler::agent_stream));
    if let Some(limiter) = &limiter {
        let limiter = limiter.clone();
        agent_stream = agent_stream.layer(middleware::from_fn(move |req, next| {
            let limiter = limiter.clone();
            async move { rate_limit::enforce(limiter, ErrorFamily::Standard, req, next).await }
        }));
    }
    let standard = Router::new()
        .route("/health", get(handler::health))
        .route("/ready", get(handler::ready))
        .route("/greeting", get(handler::greeting))
        .merge(agent_stream)
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ));

    // OpenAI-compatible endpoint (agentgateway Path C): longer timeout for the full-pipeline
    // non-streaming path (finding #1), and a dedicated bearer gate that rejects with `401` + the
    // OpenAI error envelope rather than `418` (finding #6). The shared `require_bearer` (and its
    // D6 contract for the standard endpoints above) is left untouched.
    let mut openai_routes =
        Router::new().route("/v1/chat/completions", post(handler::chat_completions));
    if let Some(limiter) = &limiter {
        // Innermost layer: a 429 is an ordinary response, so it passes back through the
        // timeout/error stack untouched; auth still runs first (outermost).
        let limiter = limiter.clone();
        openai_routes = openai_routes.layer(middleware::from_fn(move |req, next| {
            let limiter = limiter.clone();
            async move { rate_limit::enforce(limiter, ErrorFamily::OpenAi, req, next).await }
        }));
    }
    let openai = openai_routes
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
    //
    // `.fallback` is set on the **outer** router, after both merges, on purpose. `Router::merge`
    // carries a sub-router's fallback across with it, so without this the merged fallback was the
    // OpenAI group's — wrapped in that group's `require_bearer_openai`. Unmatched paths were
    // therefore auth-checked before being rejected, and answered `401` without a token but `404`
    // with a valid one: any path became an oracle for "is this token valid?", and the retired
    // paths' response depended on the `Authorization` header (AC-001 forbids exactly that). Set
    // here, the fallback sits outside both groups' auth layers, so every unmatched path answers a
    // uniform `404`.
    Router::new()
        .merge(standard)
        .merge(openai)
        .fallback(unmatched_path)
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT))
        .layer(shared)
        .with_state(state)
}

/// Uniform `404` for every unmatched path, independent of the `Authorization` header.
///
/// Deliberately body-less: a body here would be the only response shape not owned by either
/// endpoint group, and it would have to pick between the plain and the OpenAI error envelope.
async fn unmatched_path() -> StatusCode {
    StatusCode::NOT_FOUND
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
        let short =
            Router::new()
                .route("/standard", get(slow))
                .layer(TimeoutLayer::with_status_code(
                    StatusCode::GATEWAY_TIMEOUT,
                    Duration::from_millis(50),
                ));
        let long = Router::new()
            .route("/openai", get(slow))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::GATEWAY_TIMEOUT,
                Duration::from_secs(3),
            ));
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
        let app = Router::new()
            .route("/v1/chat/completions", post(slow))
            .layer(
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

    /// The four retired paths, and the header states AC-001 requires to be indistinguishable.
    const RETIRED_PATHS: [&str; 4] = ["/insight", "/insight/stream", "/report", "/report/stream"];

    /// AC-001: `POST` to a retired path answers `404`, and the answer does not depend on the
    /// `Authorization` header.
    ///
    /// The second half is the part that matters for disclosure: auth is layered onto the
    /// sub-routers, so a path that matches no route never reaches it. If a future change hangs auth
    /// off the outer router instead, an unauthenticated probe would start seeing `418` while an
    /// authenticated one saw `404` — that difference tells an attacker whether a token is valid.
    /// Asserting both header states pins the ordering, not just the status code.
    #[tokio::test]
    async fn retired_paths_return_404_regardless_of_authorization() {
        let (state, _mcp) = crate::test_support::app_state().await;
        let app = build_router(state);

        for path in RETIRED_PATHS {
            for authorization in [
                None,
                Some(format!("Bearer {}", crate::test_support::TEST_TOKEN)),
                Some("Bearer definitely-not-the-token".to_string()),
            ] {
                let mut request = Request::builder().method("POST").uri(path);
                if let Some(value) = &authorization {
                    request = request.header("authorization", value);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::from("{}")).unwrap())
                    .await
                    .unwrap();

                assert_eq!(
                    response.status(),
                    StatusCode::NOT_FOUND,
                    "{path} was retired, so it must answer 404 \
                     (authorization: {authorization:?})"
                );
            }
        }
    }

    /// The surviving paths must not be caught by the same `404` — otherwise the test above would
    /// still pass on a router that lost every route.
    #[tokio::test]
    async fn surviving_paths_are_still_routed() {
        let (state, _mcp) = crate::test_support::app_state().await;
        let app = build_router(state);

        // `/health` is unauthenticated in intent but sits behind the standard group's bearer gate,
        // so a valid token is sent. The assertion is deliberately only "not 404": this test pins
        // routing, and the handlers' own behaviour is covered elsewhere.
        for (method, path) in [
            ("GET", "/health"),
            ("GET", "/ready"),
            ("GET", "/greeting"),
            ("POST", "/agent/stream"),
            ("POST", "/v1/chat/completions"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(path)
                        .header(
                            "authorization",
                            format!("Bearer {}", crate::test_support::TEST_TOKEN),
                        )
                        .header("content-type", "application/json")
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_ne!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} survived the retirement and must still be routed"
            );
        }
    }

    /// The same uniformity, generalised past the four retired paths.
    ///
    /// The fallback fix is not specific to them: before it, *any* unmatched path answered `401`
    /// unauthenticated and `404` with a valid token, which let an unauthenticated caller test a
    /// token's validity without touching a real endpoint. This pins the general property so the
    /// oracle cannot come back through a path nobody thought to enumerate.
    #[tokio::test]
    async fn unmatched_paths_do_not_leak_token_validity() {
        let (state, _mcp) = crate::test_support::app_state().await;
        let app = build_router(state);

        for path in ["/", "/v1/models", "/nope", "/agent", "/agent/stream/extra"] {
            let mut statuses = Vec::new();
            for authorization in [
                None,
                Some(format!("Bearer {}", crate::test_support::TEST_TOKEN)),
                Some("Bearer definitely-not-the-token".to_string()),
            ] {
                let mut request = Request::builder().method("POST").uri(path);
                if let Some(value) = &authorization {
                    request = request.header("authorization", value);
                }
                let response = app
                    .clone()
                    .oneshot(request.body(Body::from("{}")).unwrap())
                    .await
                    .unwrap();
                statuses.push(response.status());
            }

            assert!(
                statuses.iter().all(|status| *status == statuses[0]),
                "{path} answered differently depending on the Authorization header \
                 ({statuses:?}), which leaks whether a token is valid"
            );
        }
    }
}
