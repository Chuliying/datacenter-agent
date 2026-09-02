//! Falcon user-identity middleware.
//!
//! The service bearer (`Authorization`) is checked by [`super::auth`] first. This layer handles
//! the end-user Falcon access token carried by `X-Falcon-Authorization`, resolves permissions, and
//! puts the opaque actor context into request extensions for the handler and runtime.

use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::error;

use super::actor::ActorKey;
use super::codes;
use super::error::ErrorBody;
use super::falcon::{Permissions, PermissionsFailure};
use super::openai::{OpenAiErrorBody, ERR_INVALID_REQUEST};
use super::AppState;
use crate::runtime::audit::{
    AuditCtx, AuditEvent, AuditFailurePolicy, AuditSink, AuditWriter, TracingAuditSink,
};

/// Header carrying the Falcon access token delegated by the caller.
pub const FALCON_AUTHORIZATION_HEADER: &str = "x-falcon-authorization";

/// Request-scoped, verified Falcon identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityContext {
    /// Opaque, peppered actor identifier used for rate limits, memory, and audit.
    pub actor_key: ActorKey,
    /// The effective Falcon permissions for this request.
    pub permissions: Permissions,
}

/// Resolve the Falcon identity for a prompt request.
///
/// Non-POST requests are passed through before looking at the identity header. The method router
/// therefore retains its existing `405` behavior and probes cannot consume identity or limiter
/// capacity merely by using the wrong method.
pub async fn enforce(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    if req.method() != axum::http::Method::POST {
        return next.run(req).await;
    }

    let Some(token) = req
        .headers()
        .get(FALCON_AUTHORIZATION_HEADER)
        .and_then(parse_bearer)
        .map(str::to_owned)
    else {
        audit_identity_refused(&state, req.uri().path().to_string(), "header_missing").await;
        return identity_error(
            &req,
            StatusCode::UNAUTHORIZED,
            codes::IDENTITY_HEADER_MISSING,
            "missing or malformed X-Falcon-Authorization header",
        );
    };

    let permissions = match state.permissions_provider.permissions(&token).await {
        Ok(permissions) => permissions,
        Err(PermissionsFailure::UnauthorizedRefreshable) => {
            audit_identity_refused(&state, req.uri().path().to_string(), "token_refreshable").await;
            return identity_error(
                &req,
                StatusCode::UNAUTHORIZED,
                codes::IDENTITY_TOKEN_REFRESHABLE,
                "Falcon access token expired; refresh and retry once",
            );
        }
        Err(PermissionsFailure::UnauthorizedTerminal) => {
            audit_identity_refused(&state, req.uri().path().to_string(), "token_terminal").await;
            return identity_error(
                &req,
                StatusCode::UNAUTHORIZED,
                codes::IDENTITY_TOKEN_TERMINAL,
                "Falcon access token cannot be used and refreshing will not help",
            );
        }
        Err(PermissionsFailure::UnauthorizedPlatformCanary) => {
            // Terminal for the caller, canary for us: the endpoint verifies the token's
            // platform claim, and this design forwards browser-issued tokens on the strength
            // of one ops confirmation. This code showing up means that allowlist changed.
            audit_identity_alarm(&state, req.uri().path().to_string(), "invalid_platform").await;
            return identity_error(
                &req,
                StatusCode::UNAUTHORIZED,
                codes::IDENTITY_TOKEN_TERMINAL,
                "Falcon access token cannot be used and refreshing will not help",
            );
        }
        Err(PermissionsFailure::UpstreamConflict) => {
            // We only ever send a Bearer header, so a conflicting-credentials report is a
            // runtime request-construction bug — 500, alarmed, and never negative-cached.
            audit_identity_alarm(&state, req.uri().path().to_string(), "upstream_conflict").await;
            return identity_error(
                &req,
                StatusCode::INTERNAL_SERVER_ERROR,
                codes::IDENTITY_UPSTREAM_CONFLICT,
                "identity verification failed on the runtime side",
            );
        }
        Err(PermissionsFailure::Unavailable) => {
            audit_identity_refused(&state, req.uri().path().to_string(), "upstream_unavailable")
                .await;
            return identity_error(
                &req,
                StatusCode::SERVICE_UNAVAILABLE,
                codes::IDENTITY_UPSTREAM_UNAVAILABLE,
                "Falcon permissions service is temporarily unavailable",
            );
        }
    };

    let actor_key = match ActorKey::derive(permissions.user_id, &state.actor_key_pepper) {
        Ok(actor_key) => actor_key,
        Err(err) => {
            // This is a boot-time invariant in production. Keep the request fail-closed if a
            // hand-built state violates it in a test or an embedding application.
            error!(error = %err, "identity: actor key derivation failed");
            audit_identity_alarm(
                &state,
                req.uri().path().to_string(),
                "actor_key_derivation_failed",
            )
            .await;
            return identity_error(
                &req,
                StatusCode::SERVICE_UNAVAILABLE,
                codes::IDENTITY_UPSTREAM_UNAVAILABLE,
                "identity service is unavailable",
            );
        }
    };

    req.extensions_mut().insert(IdentityContext {
        actor_key,
        permissions,
    });
    next.run(req).await
}

/// Parse exactly the `Bearer <token>` transport documented by Falcon.
fn parse_bearer(value: &HeaderValue) -> Option<&str> {
    let value = value.to_str().ok()?.trim();
    let (scheme, token) = value.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty() && !token.chars().any(char::is_whitespace)).then_some(token)
}

fn identity_error(req: &Request, status: StatusCode, code: &str, message: &str) -> Response {
    if req.uri().path().starts_with("/v1/") {
        (
            status,
            Json(OpenAiErrorBody::with_code(
                code,
                ERR_INVALID_REQUEST,
                message,
            )),
        )
            .into_response()
    } else {
        (status, Json(ErrorBody::with_code(code, message))).into_response()
    }
}

async fn audit_identity_refused(state: &AppState, route: String, kind: &str) {
    audit_identity_event(state, route, kind, false).await;
}

async fn audit_identity_alarm(state: &AppState, route: String, kind: &str) {
    audit_identity_event(state, route, kind, true).await;
}

async fn audit_identity_event(state: &AppState, route: String, kind: &str, alarm: bool) {
    let sink: std::sync::Arc<dyn AuditSink> = state
        .runtime
        .as_ref()
        .map(|runtime| runtime.audit_sink.clone())
        .unwrap_or_else(|| std::sync::Arc::new(TracingAuditSink));
    let writer = AuditWriter::new(sink, AuditFailurePolicy::FailOpen);
    let ctx = AuditCtx {
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: None,
        route,
        actor_key: None,
        actor: None,
    };
    let event = if alarm {
        AuditEvent::IdentityAlarm {
            kind: kind.to_string(),
        }
    } else {
        AuditEvent::IdentityRefused {
            kind: kind.to_string(),
        }
    };
    if let Err(err) = writer.write(&ctx, event).await {
        error!(error = %err, "identity: alarm audit write failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::falcon::PermissionsProvider;
    use axum::body::Body;
    use axum::extract::Extension;
    use axum::http::Request;
    use axum::middleware;
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use tower::ServiceExt;

    fn header(value: &str) -> HeaderValue {
        HeaderValue::from_str(value).expect("test header should be valid")
    }

    #[test]
    fn bearer_parser_accepts_case_insensitive_scheme_and_rejects_bad_values() {
        assert_eq!(parse_bearer(&header("Bearer token-123")), Some("token-123"));
        assert_eq!(parse_bearer(&header("bEaReR token-123")), Some("token-123"));
        assert_eq!(parse_bearer(&header("Basic token-123")), None);
        assert_eq!(parse_bearer(&header("Bearer")), None);
        assert_eq!(parse_bearer(&header("Bearer  ")), None);
        assert_eq!(parse_bearer(&header("Bearer token with-space")), None);
    }

    async fn identity_echo(
        Extension(identity): Extension<IdentityContext>,
    ) -> Json<serde_json::Value> {
        Json(json!({
            "actor_key": identity.actor_key.as_str(),
            "user_id": identity.permissions.user_id,
            "permissions": identity.permissions.codes,
        }))
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body should be readable");
        serde_json::from_slice(&body).expect("response should be JSON")
    }

    fn request(path: &str, falcon_header: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json");
        if let Some(value) = falcon_header {
            builder = builder.header(FALCON_AUTHORIZATION_HEADER, value);
        }
        builder
            .body(Body::empty())
            .expect("test request should build")
    }

    async fn identity_app(provider: std::sync::Arc<dyn PermissionsProvider>) -> axum::Router {
        let (state, _mcp) = crate::test_support::app_state_with_provider(provider).await;
        Router::new()
            .route("/prompt", post(identity_echo))
            .route("/v1/prompt", post(identity_echo))
            .layer(middleware::from_fn_with_state(state.clone(), enforce))
            .with_state(state)
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-010, AC-011
    async fn identity_middleware_distinguishes_missing_unauthorized_and_unavailable() {
        let cases = [
            (
                None,
                StatusCode::UNAUTHORIZED,
                codes::IDENTITY_HEADER_MISSING,
            ),
            (
                Some("Bearer bad-token"),
                StatusCode::UNAUTHORIZED,
                codes::IDENTITY_TOKEN_TERMINAL,
            ),
            (
                Some("Bearer falcon-down"),
                StatusCode::SERVICE_UNAVAILABLE,
                codes::IDENTITY_UPSTREAM_UNAVAILABLE,
            ),
        ];
        let provider = crate::test_support::ScriptedPermissionsProvider::new([
            Err(PermissionsFailure::UnauthorizedTerminal),
            Err(PermissionsFailure::Unavailable),
        ]);
        let (state, _mcp) = crate::test_support::runtime_app_state(provider.clone()).await;
        let app = Router::new()
            .route("/prompt", post(identity_echo))
            .layer(middleware::from_fn_with_state(state.clone(), enforce))
            .with_state(state);

        for (header, status, code) in cases {
            let response = app
                .clone()
                .oneshot(request("/prompt", header))
                .await
                .expect("identity request should complete");
            assert_eq!(response.status(), status);
            let body = response_json(response).await;
            assert_eq!(body["code"], code);
        }
        assert_eq!(provider.calls(), 2, "missing header must not call Falcon");
    }

    /// AC-002 (the tracing half): the end user's Falcon token must not reach the log stream.
    ///
    /// The cache half is pinned in `falcon.rs` and the audit half in `audit.rs`; this covers the
    /// one that a `tracing::info!` added in passing would silently break. The capture layer is
    /// installed process-wide precisely so it also sees output from spawned tasks — a per-test
    /// scoped subscriber would miss exactly the emissions worth worrying about.
    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-002
    async fn ac002_user_token_never_reaches_the_tracing_stream() {
        const SECRET: &str = "falcon-token-that-must-never-be-logged-9f2c";

        let buffer = crate::test_support::install_tracing_capture();
        let granted = crate::server::falcon::Permissions {
            user_id: 42,
            codes: ["hdrenewables/elecsvc/starcharger/finance".to_string()]
                .into_iter()
                .collect(),
        };
        let app = identity_app(crate::test_support::ScriptedPermissionsProvider::new([
            Ok(granted),
            Err(crate::server::falcon::PermissionsFailure::UnauthorizedTerminal),
        ]))
        .await;

        // A success and a failure: both paths log, and the failure path is the one that handles
        // the token most directly.
        let ok = app
            .clone()
            .oneshot(request("/prompt", Some(&format!("Bearer {SECRET}"))))
            .await
            .expect("request should complete");
        assert_eq!(ok.status(), StatusCode::OK);

        let denied = app
            .oneshot(request("/prompt", Some(&format!("Bearer {SECRET}-other"))))
            .await
            .expect("request should complete");
        assert_ne!(denied.status(), StatusCode::OK);

        let captured = String::from_utf8_lossy(
            &buffer
                .lock()
                .expect("tracing capture lock should not be poisoned")
                .clone(),
        )
        .into_owned();

        assert!(
            !captured.contains(SECRET),
            "the user token leaked into tracing output"
        );
    }

    #[tokio::test]
    async fn identity_middleware_derives_opaque_actor_and_inserts_permissions() {
        let provider = crate::test_support::ScriptedPermissionsProvider::documented(
            123,
            &["hdrenewables/elecsvc/starcharger/finance"],
        );
        let app = identity_app(provider.clone()).await;
        let response = app
            .oneshot(request("/prompt", Some("bEaReR delegated-token")))
            .await
            .expect("identity request should complete");

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["user_id"], 123);
        assert_eq!(
            body["permissions"][0],
            "hdrenewables/elecsvc/starcharger/finance"
        );
        assert_eq!(provider.calls(), 1);
        let actor = body["actor_key"]
            .as_str()
            .expect("actor key should be text");
        assert!(actor.starts_with("v1:"));
        assert!(!actor.contains("123"), "actor key must not expose user id");
        assert!(
            !actor.contains("delegated-token"),
            "token must not enter actor key"
        );
    }

    #[tokio::test]
    async fn identity_passes_non_post_through_without_calling_falcon() {
        let provider = crate::test_support::ScriptedPermissionsProvider::documented(123, &[]);
        let (state, _mcp) = crate::test_support::app_state_with_provider(provider.clone()).await;
        let app = Router::new()
            .route("/prompt", post(identity_echo))
            .layer(middleware::from_fn_with_state(state.clone(), enforce))
            .with_state(state);
        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/prompt")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("GET should reach method routing");

        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        assert_eq!(provider.calls(), 0);
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-016
    async fn identity_uses_openai_error_envelope_for_v1_paths() {
        let provider = crate::test_support::ScriptedPermissionsProvider::new([Err(
            PermissionsFailure::UnauthorizedTerminal,
        )]);
        let app = identity_app(provider).await;
        let response = app
            .oneshot(request("/v1/prompt", Some("Bearer expired")))
            .await
            .expect("identity request should complete");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["error"]["type"], ERR_INVALID_REQUEST);
        assert_eq!(body["error"]["code"], codes::IDENTITY_TOKEN_TERMINAL);
    }
}
