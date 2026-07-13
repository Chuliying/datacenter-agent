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

use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use super::auth::require_bearer;
use super::handler;
use super::AppState;

/// 64 KiB. Defense-in-depth above the per-field 2 000-char prompt cap.
const REQUEST_BODY_LIMIT: usize = 64 * 1024;

/// 120 s. The slow path is the LLM round-trip.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub fn build_router(state: AppState) -> Router {
    let middleware = ServiceBuilder::new()
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::very_permissive())
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ));

    // Routes added before the bearer-auth layer inherit it, and anything added after
    // `.layer(...)` below would silently bypass auth.
    Router::new()
        .route("/health", get(handler::health))
        .route("/ready", get(handler::ready))
        .route("/greeting", get(handler::greeting))
        .route("/insight", post(handler::insight))
        .route("/insight/stream", post(handler::insight_stream))
        .route("/report", post(handler::report))
        .route("/report/stream", post(handler::report_stream))
        .route("/agent/stream", post(handler::agent_stream))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_bearer,
        ))
        .layer(DefaultBodyLimit::max(REQUEST_BODY_LIMIT))
        .layer(middleware)
        .with_state(state)
}
