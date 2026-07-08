//! Runtime audit events and sinks.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tracing::error;

use super::error::{RuntimeError, RuntimeResult};

/// Audit sink failure behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditFailurePolicy {
    /// Log audit failures but continue request processing.
    FailOpen,
    /// Abort request processing when audit write fails.
    FailClosed,
}

/// Runtime audit event.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEvent {
    /// Request was received.
    RequestReceived {
        /// Hash of the raw input.
        input_hash: String,
        /// Raw input length.
        input_chars: usize,
        /// Optional frontend option id.
        option_id: Option<String>,
    },
    /// Input was normalized.
    InputNormalized {
        /// Selected intent.
        intent: String,
        /// Intent confidence.
        confidence: f32,
        /// Intent source.
        intent_source: Option<String>,
        /// Warnings.
        warnings: Vec<String>,
    },
    /// Input was rejected.
    InputRejected {
        /// Rejection code.
        code: String,
        /// Rejection reason.
        reason: String,
    },
    /// Semantic refusal.
    Refused {
        /// Refusal reason.
        reason: String,
    },
    /// Memory context decision.
    MemoryContext {
        /// Used memory turn count.
        used_turn_count: usize,
        /// Optional dropped reason.
        dropped_reason: Option<String>,
    },
    /// Tool call metadata.
    ToolCalled {
        /// Tool name.
        tool: String,
        /// Hash of arguments.
        args_hash: String,
    },
    /// Tool result metadata.
    ToolResult {
        /// Tool name.
        tool: String,
        /// Result byte length.
        bytes: usize,
        /// Whether the tool returned successfully.
        ok: bool,
    },
    /// Clear frame observed.
    AnswerCleared,
    /// Response completed.
    ResponseCompleted {
        /// Hash of response text.
        response_hash: String,
        /// Response length.
        response_chars: usize,
        /// Duration in milliseconds.
        duration_ms: u64,
        /// Completion status.
        status: String,
    },
    /// Response failed.
    ResponseFailed {
        /// Error code.
        error_code: String,
        /// Duration in milliseconds.
        duration_ms: u64,
    },
}

/// Actor metadata attached to audit records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditActor {
    /// Remote IP address.
    pub ip: Option<String>,
    /// User-Agent header.
    pub user_agent: Option<String>,
}

/// Request-scoped audit context.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditCtx {
    /// Request id.
    pub request_id: String,
    /// Optional session id.
    pub session_id: Option<String>,
    /// Route name/path.
    pub route: String,
    /// Actor metadata.
    pub actor: Option<AuditActor>,
}

/// Serializable audit record emitted by sinks.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditRecord {
    /// Request id.
    pub request_id: String,
    /// Optional session id.
    pub session_id: Option<String>,
    /// Route name/path.
    pub route: String,
    /// Per-request monotonic sequence number.
    pub seq: u64,
    /// Hashed actor IP.
    pub actor_ip: Option<String>,
    /// Hashed actor user agent.
    pub actor_user_agent: Option<String>,
    /// Event payload.
    pub event: AuditEvent,
}

impl AuditRecord {
    /// Build an audit record from request context and event payload.
    pub fn from_event(ctx: &AuditCtx, seq: u64, event: AuditEvent) -> Self {
        Self {
            request_id: ctx.request_id.clone(),
            session_id: ctx.session_id.clone(),
            route: ctx.route.clone(),
            seq,
            actor_ip: ctx
                .actor
                .as_ref()
                .and_then(|actor| actor.ip.as_ref())
                .map(|value| hash_identifier(value)),
            actor_user_agent: ctx
                .actor
                .as_ref()
                .and_then(|actor| actor.user_agent.as_ref())
                .map(|value| hash_identifier(value)),
            event,
        }
    }
}

/// Audit sink contract.
#[async_trait]
pub trait AuditSink: Send + Sync {
    /// Write one audit event.
    async fn write(&self, ctx: &AuditCtx, seq: u64, event: AuditEvent) -> RuntimeResult<()>;
}

/// No-op audit sink for disabled or placeholder wiring.
#[derive(Debug, Default)]
pub struct NoopAuditSink;

#[async_trait]
impl AuditSink for NoopAuditSink {
    async fn write(&self, _ctx: &AuditCtx, _seq: u64, _event: AuditEvent) -> RuntimeResult<()> {
        Ok(())
    }
}

/// Stdout JSON-lines audit sink.
#[derive(Debug, Default)]
pub struct StdoutAuditSink;

#[async_trait]
impl AuditSink for StdoutAuditSink {
    async fn write(&self, ctx: &AuditCtx, seq: u64, event: AuditEvent) -> RuntimeResult<()> {
        let record = AuditRecord::from_event(ctx, seq, event);
        let json = serde_json::to_string(&record)
            .map_err(|err| RuntimeError::AuditSink(format!("serialize audit record: {err}")))?;
        println!("{json}");
        Ok(())
    }
}

/// Request-scoped audit writer that assigns monotonic sequence numbers and
/// applies the configured failure policy.
pub struct AuditWriter {
    sink: Arc<dyn AuditSink>,
    policy: AuditFailurePolicy,
    seq: AtomicU64,
}

impl AuditWriter {
    /// Create a writer for one request.
    pub fn new(sink: Arc<dyn AuditSink>, policy: AuditFailurePolicy) -> Self {
        Self {
            sink,
            policy,
            seq: AtomicU64::new(0),
        }
    }

    /// Write an event using the next sequence number.
    pub async fn write(&self, ctx: &AuditCtx, event: AuditEvent) -> RuntimeResult<()> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        match self.sink.write(ctx, seq, event).await {
            Ok(()) => Ok(()),
            Err(err) if self.policy == AuditFailurePolicy::FailOpen => {
                error!(error = %err, "runtime audit write failed; continuing because fail-open");
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

/// Stable SHA-256 hex hash helper for redacted identifiers.
pub fn hash_identifier(input: &str) -> String {
    format!("{:x}", Sha256::digest(input.as_bytes()))
}

/// Redact known secret patterns from audit-safe strings.
pub fn redact_secrets(input: &str) -> String {
    let redacted = match regex::Regex::new(r"(?i)bearer\s+[A-Za-z0-9._~+/=-]+") {
        Ok(bearer) => bearer.replace_all(input, "Bearer [REDACTED]").into_owned(),
        Err(_) => input.to_string(),
    };
    match regex::Regex::new(r"(?i)\b(GLOBAL_TOKEN|OPENROUTER_API_KEY|api[_-]?key)\s*=\s*\S+") {
        Ok(env_secret) => env_secret
            .replace_all(&redacted, "$1=[REDACTED]")
            .into_owned(),
        Err(_) => redacted,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::*;
    use crate::runtime::error::RuntimeError;

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

    #[derive(Debug, Default)]
    struct FailingSink;

    #[async_trait]
    impl AuditSink for FailingSink {
        async fn write(&self, _ctx: &AuditCtx, _seq: u64, _event: AuditEvent) -> RuntimeResult<()> {
            Err(RuntimeError::AuditSink("disk full".into()))
        }
    }

    fn ctx() -> AuditCtx {
        AuditCtx {
            request_id: "req-1".into(),
            session_id: Some("session-1".into()),
            route: "/agent".into(),
            actor: Some(AuditActor {
                ip: Some("203.0.113.9".into()),
                user_agent: Some("Bearer secret-user-agent".into()),
            }),
        }
    }

    #[tokio::test]
    async fn audit_writer_assigns_monotonic_seq_and_redacts_actor() {
        let sink = Arc::new(CapturingSink::default());
        let writer = AuditWriter::new(sink.clone(), AuditFailurePolicy::FailClosed);

        writer
            .write(
                &ctx(),
                AuditEvent::RequestReceived {
                    input_hash: hash_identifier("prompt"),
                    input_chars: 6,
                    option_id: None,
                },
            )
            .await
            .expect("audit should write");
        writer
            .write(
                &ctx(),
                AuditEvent::ResponseCompleted {
                    response_hash: hash_identifier("answer"),
                    response_chars: 6,
                    duration_ms: 10,
                    status: "completed".into(),
                },
            )
            .await
            .expect("audit should write");

        let records = sink.records.lock().await;
        assert_eq!(records[0].seq, 1);
        assert_eq!(records[1].seq, 2);
        assert_eq!(records[0].request_id, "req-1");
        assert_eq!(
            records[0].actor_ip.as_deref(),
            Some(hash_identifier("203.0.113.9").as_str())
        );
        assert_eq!(
            records[0].actor_user_agent.as_deref(),
            Some(hash_identifier("Bearer secret-user-agent").as_str())
        );
    }

    #[tokio::test]
    async fn audit_failure_policy_fail_open_continues() {
        let writer = AuditWriter::new(Arc::new(FailingSink), AuditFailurePolicy::FailOpen);

        let result = writer.write(&ctx(), AuditEvent::AnswerCleared).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn audit_failure_policy_fail_closed_returns_error() {
        let writer = AuditWriter::new(Arc::new(FailingSink), AuditFailurePolicy::FailClosed);

        let result = writer.write(&ctx(), AuditEvent::AnswerCleared).await;

        assert!(matches!(result, Err(RuntimeError::AuditSink(_))));
    }

    #[test]
    fn redact_secrets_masks_known_tokens() {
        let input = "Bearer abc OPENROUTER_API_KEY=sk-live GLOBAL_TOKEN=secret api_key=test";

        let redacted = redact_secrets(input);

        assert!(!redacted.contains("sk-live"));
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("abc"));
        assert!(redacted.contains("[REDACTED]"));
    }
}
