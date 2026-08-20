//! Opt-in process-local global burst limiter (S-RUNTIME-SEC-01 FR-005).
//!
//! One token-bucket policy guards the expensive routes (`/agent/stream`,
//! `/v1/chat/completions`), positioned *inside* each route family's bearer
//! gate (auth runs first — AC-014) and before JSON extraction and handlers.
//! A rejection answers `429` with an integer `Retry-After`, `Cache-Control:
//! no-store`, the pinned per-family error body (AC-015), and exactly one
//! structured [`AuditEvent::RateLimitRejected`] through the existing sink —
//! never a SQLite write (AC-010).
//!
//! State is process-local (`governor` token bucket) and resets on restart:
//! this is short-window admission control, not the durable monthly ledger.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use governor::clock::{Clock, QuantaClock};
use governor::middleware::NoOpMiddleware;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};

use crate::config::RateLimitConfig;
use crate::runtime::audit::{AuditCtx, AuditEvent, AuditSink};

use super::error::ErrorBody;
use super::openai::{OpenAiErrorBody, ERR_RATE_LIMIT};

/// Which pinned `429` envelope a route family answers with (AC-015).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorFamily {
    /// `{"error": "<message>"}` — the uniform envelope of the standard routes.
    Standard,
    /// `{"error": {"message", "type": "rate_limit_error"}}` — the OpenAI envelope.
    OpenAi,
}

type DirectLimiter = RateLimiter<
    NotKeyed,
    InMemoryState,
    QuantaClock,
    NoOpMiddleware<governor::clock::QuantaInstant>,
>;

/// Shared limiter state: one global bucket for every expensive route.
pub struct GlobalBurstLimiter {
    limiter: DirectLimiter,
    clock: QuantaClock,
    audit: Arc<dyn AuditSink>,
    policy_version: String,
}

impl GlobalBurstLimiter {
    /// Build the limiter from an **enabled** config and the existing audit sink.
    pub fn new(config: &RateLimitConfig, audit: Arc<dyn AuditSink>) -> Self {
        let clock = QuantaClock::default();
        let period = std::time::Duration::from_millis(config.refill_period_ms.get());
        let quota = Quota::with_period(period)
            .expect("refill period is non-zero by construction")
            .allow_burst(config.burst_size);
        Self {
            limiter: RateLimiter::direct_with_clock(quota, clock.clone()),
            clock,
            audit,
            policy_version: format!(
                "burst={};refill_ms={}",
                config.burst_size, config.refill_period_ms
            ),
        }
    }
}

/// Admit or reject one request. Attach via
/// `middleware::from_fn(move |req, next| enforce(limiter.clone(), family, req, next))`
/// *inside* the route family's bearer layer (auth stays outermost, D-006).
pub async fn enforce(
    limiter: Arc<GlobalBurstLimiter>,
    family: ErrorFamily,
    req: Request,
    next: Next,
) -> Response {
    let not_until = match limiter.limiter.check() {
        Ok(_) => return next.run(req).await,
        Err(not_until) => not_until,
    };

    // Integer delta-seconds, rounded up so "retry after n" is never early;
    // floor at 1 so the header is a meaningful delay.
    let wait = not_until.wait_time_from(limiter.clock.now());
    let retry_after_secs = wait
        .as_secs()
        .saturating_add(u64::from(wait.subsec_nanos() > 0))
        .max(1);

    // Exactly one bounded audit event per rejection, through the existing
    // sink — never a SQLite write (AC-010, D-007). Awaited inline: the sinks
    // are cheap, and the write must not race the response in tests.
    let ctx = AuditCtx {
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: None,
        route: req.uri().path().to_string(),
        actor: None,
    };
    let event = AuditEvent::RateLimitRejected {
        decision: "rejected".to_string(),
        retry_after_secs,
        policy_version: limiter.policy_version.clone(),
    };
    if let Err(err) = limiter.audit.write(&ctx, 1, event).await {
        tracing::error!(error = %err, "rate-limit audit write failed; rejection continues");
    }

    let message = format!("rate limited: retry after {retry_after_secs}s");
    let body = match family {
        ErrorFamily::Standard => Json(ErrorBody::new(message)).into_response(),
        ErrorFamily::OpenAi => Json(OpenAiErrorBody::new(ERR_RATE_LIMIT, message)).into_response(),
    };
    let mut response = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&retry_after_secs.to_string())
            .expect("integer seconds are a valid header value"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU32, NonZeroU64};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::middleware;
    use axum::routing::post;
    use axum::Router;
    use tokio::sync::Mutex;
    use tower::ServiceExt;

    use super::*;
    use crate::runtime::audit::AuditRecord;
    use crate::runtime::error::RuntimeResult;
    use crate::server::route::build_router;

    fn enabled_policy(burst: u32) -> RateLimitConfig {
        RateLimitConfig {
            enabled: true,
            burst_size: NonZeroU32::new(burst).unwrap(),
            // One hour per token: within a test, the bucket never refills,
            // so every assertion is deterministic without wall-clock sleeps.
            refill_period_ms: NonZeroU64::new(3_600_000).unwrap(),
        }
    }

    /// Full app with the limiter enabled at the given burst.
    async fn limited_app(burst: u32) -> Router {
        let (mut state, _mcp) = crate::test_support::app_state().await;
        state.rate_limit = enabled_policy(burst);
        build_router(state)
    }

    fn authed(path: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri(path)
            .header(
                "authorization",
                format!("Bearer {}", crate::test_support::TEST_TOKEN),
            )
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap()
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).expect("429 body must be JSON")
    }

    fn assert_429_headers(resp: &Response) {
        let retry = resp
            .headers()
            .get(header::RETRY_AFTER)
            .expect("429 must carry Retry-After")
            .to_str()
            .unwrap()
            .to_string();
        retry
            .parse::<u64>()
            .expect("Retry-After must be integer delta-seconds");
        assert_eq!(
            resp.headers()
                .get(header::CACHE_CONTROL)
                .map(|v| v.to_str().unwrap()),
            Some("no-store"),
            "429 must not be cacheable (RFC 6585 §4)"
        );
    }

    /// AC-008: with the bucket exhausted, the next authenticated request gets
    /// `429` before any handler runs (the live handler would answer 503 here).
    #[tokio::test]
    async fn ac008_burst_rejection_happens_before_the_handler() {
        let app = limited_app(1).await;

        let first = app.clone().oneshot(authed("/agent/stream")).await.unwrap();
        assert_ne!(
            first.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "first admits"
        );

        let second = app.oneshot(authed("/agent/stream")).await.unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_429_headers(&second);
        let body = body_json(second).await;
        assert!(body["error"].is_string(), "standard envelope, got {body}");
    }

    /// AC-009: probes and greeting stay outside the exhausted bucket, with the
    /// same status a limiter-free router answers (TC-009 baseline comparison).
    #[tokio::test]
    async fn ac009_probe_and_greeting_routes_are_not_limited() {
        let (baseline_state, _baseline_mcp) = crate::test_support::app_state().await;
        let baseline = build_router(baseline_state);
        let app = limited_app(1).await;
        let _ = app.clone().oneshot(authed("/agent/stream")).await.unwrap();
        let blocked = app.clone().oneshot(authed("/agent/stream")).await.unwrap();
        assert_eq!(
            blocked.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "bucket empty"
        );

        for path in ["/health", "/ready", "/greeting"] {
            let probe = || {
                HttpRequest::builder()
                    .method("GET")
                    .uri(path)
                    .header(
                        "authorization",
                        format!("Bearer {}", crate::test_support::TEST_TOKEN),
                    )
                    .body(Body::empty())
                    .unwrap()
            };
            let limited = app.clone().oneshot(probe()).await.unwrap();
            let unlimited = baseline.clone().oneshot(probe()).await.unwrap();
            assert_eq!(
                limited.status(),
                unlimited.status(),
                "{path} must answer exactly as it does without a limiter"
            );
        }
    }

    /// AC-014: a request failing the bearer contract is rejected first and
    /// consumes no admission capacity.
    #[tokio::test]
    async fn ac014_unauthenticated_traffic_cannot_starve_the_bucket() {
        let app = limited_app(1).await;

        let unauthed = HttpRequest::builder()
            .method("POST")
            .uri("/agent/stream")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let rejected = app.clone().oneshot(unauthed).await.unwrap();
        assert_eq!(
            rejected.status(),
            StatusCode::IM_A_TEAPOT,
            "auth rejects first"
        );

        let admitted = app.oneshot(authed("/agent/stream")).await.unwrap();
        assert_ne!(
            admitted.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "the teapot must not have consumed the only slot"
        );
    }

    /// AC-014, OpenAI family: a bad bearer answers `401` before the limiter
    /// and consumes no capacity either.
    #[tokio::test]
    async fn ac014_openai_unauthenticated_rejects_401_without_consuming() {
        let app = limited_app(1).await;

        let unauthed = HttpRequest::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let rejected = app.clone().oneshot(unauthed).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        let admitted = app.oneshot(authed("/v1/chat/completions")).await.unwrap();
        assert_ne!(admitted.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    /// AC-015: the `429` body is pinned per route family.
    #[tokio::test]
    async fn ac015_429_body_is_pinned_per_route_family() {
        let app = limited_app(1).await;
        let _ = app.clone().oneshot(authed("/agent/stream")).await.unwrap();

        let standard = app.clone().oneshot(authed("/agent/stream")).await.unwrap();
        assert_eq!(standard.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_429_headers(&standard);
        let body = body_json(standard).await;
        assert!(
            body["error"].is_string(),
            "standard: {{\"error\": string}}, got {body}"
        );

        let openai = app.oneshot(authed("/v1/chat/completions")).await.unwrap();
        assert_eq!(openai.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_429_headers(&openai);
        let body = body_json(openai).await;
        assert_eq!(body["error"]["type"], "rate_limit_error");
        assert!(body["error"]["message"].is_string(), "got {body}");
    }

    #[derive(Debug, Default)]
    struct CapturingSink {
        records: Mutex<Vec<AuditRecord>>,
    }

    #[async_trait]
    impl AuditSink for CapturingSink {
        async fn write(&self, ctx: &AuditCtx, seq: u64, event: AuditEvent) -> RuntimeResult<()> {
            self.records
                .lock()
                .await
                .push(AuditRecord::from_event(ctx, seq, event));
            Ok(())
        }
    }

    /// AC-010 + handler-spy: one rejection emits exactly one bounded audit
    /// event and never invokes the handler.
    #[tokio::test]
    async fn ac010_rejection_audits_once_with_bounded_fields_and_skips_handler() {
        let sink = Arc::new(CapturingSink::default());
        let limiter = Arc::new(GlobalBurstLimiter::new(
            &enabled_policy(1),
            sink.clone() as Arc<dyn AuditSink>,
        ));
        let hits = Arc::new(AtomicUsize::new(0));
        let handler_hits = hits.clone();
        let app = Router::new()
            .route(
                "/expensive",
                post(move || {
                    let hits = handler_hits.clone();
                    async move {
                        hits.fetch_add(1, Ordering::SeqCst);
                        "ok"
                    }
                }),
            )
            .layer(middleware::from_fn(move |req: Request, next: Next| {
                let limiter = limiter.clone();
                async move { enforce(limiter, ErrorFamily::Standard, req, next).await }
            }));

        let request = || {
            HttpRequest::builder()
                .method("POST")
                .uri("/expensive")
                .body(Body::from(
                    "raw prompt with Bearer sk-secret and ip 203.0.113.9",
                ))
                .unwrap()
        };
        let first = app.clone().oneshot(request()).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let second = app.oneshot(request()).await.unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);

        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "rejected request skips the handler"
        );

        let records = sink.records.lock().await;
        assert_eq!(records.len(), 1, "exactly one audit event per rejection");
        let record = &records[0];
        assert!(!record.request_id.is_empty());
        assert_eq!(record.route, "/expensive");
        assert_eq!(record.session_id, None, "no session in limiter audit");
        assert_eq!(record.actor_ip, None, "no IP in limiter audit");
        let AuditEvent::RateLimitRejected {
            ref decision,
            retry_after_secs,
            ref policy_version,
        } = record.event
        else {
            panic!("expected RateLimitRejected, got {:?}", record.event);
        };
        assert_eq!(decision, "rejected");
        assert!(retry_after_secs >= 1);
        assert_eq!(policy_version, "burst=1;refill_ms=3600000");
        let serialized = serde_json::to_string(record).unwrap();
        assert!(
            !serialized.contains("sk-secret"),
            "audit must not carry body content"
        );
        assert!(
            !serialized.contains("203.0.113.9"),
            "audit must not carry IPs"
        );
    }

    /// Disabled config leaves both route families unlimited (opt-in default).
    #[tokio::test]
    async fn disabled_config_attaches_no_limiter() {
        let (state, _mcp) = crate::test_support::app_state().await;
        assert!(
            !state.rate_limit.enabled,
            "fixture default must be disabled"
        );
        let app = build_router(state);
        for path in ["/agent/stream", "/v1/chat/completions"] {
            for _ in 0..3 {
                let resp = app.clone().oneshot(authed(path)).await.unwrap();
                assert_ne!(resp.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
            }
        }
    }
}
