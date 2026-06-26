//! Runtime turn orchestrator.

use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_openai::types::chat::ChatCompletionTool;
use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};

use super::audit::{hash_identifier, AuditCtx, AuditEvent, AuditWriter};
use super::config::RuntimeConfig;
use super::error::{RuntimeError, RuntimeResult};
use super::guardrails::answer_policy::{AnswerDecision, AnswerPolicy};
use super::guardrails::input_guard::validate_prompt;
use super::input::pipeline::InputPipeline;
use super::llm_normalizer::LlmInputNormalizer;
use super::memory::context::build_session_memory_context;
use super::memory::store::{SessionMemoryScope, SessionMemoryStore, SessionMemoryTurn};
use super::schema::{AgentTurnFrame, AgentTurnInput, NormalizedInput};
use crate::llm_connector::{self, LlmEvent};
use crate::mcp_client::McpHandle;
use crate::model::GenerationConfig;

/// Agent transport port consumed by the orchestrator.
#[async_trait]
pub trait AgentPort: Send + Sync {
    /// Start one model/tool turn.
    async fn stream_turn(
        &self,
        input: AgentTurnInput,
    ) -> RuntimeResult<BoxStream<'static, AgentTurnFrame>>;
}

/// Runtime orchestrator placeholder.
#[derive(Debug, Default)]
pub struct RuntimeOrchestrator;

/// Outcome of one runtime-owned turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTurnOutcome {
    /// Final answer.
    Final {
        /// Final response text.
        response: String,
        /// Selected intent.
        intent: String,
    },
    /// Semantic refusal.
    Refused {
        /// Refusal reason.
        reason: String,
        /// Refusal copy.
        copy: String,
    },
    /// Request/upstream error.
    Error {
        /// Stable error code.
        code: String,
        /// HTTP-ish status for host mapping.
        status: u16,
    },
    /// Stream aborted after partial output.
    Aborted {
        /// Partial response.
        response: String,
    },
}

/// Runtime dependencies for one turn.
#[derive(Clone, Copy)]
pub struct AgentTurnDeps<'a> {
    /// Runtime config.
    pub runtime_config: &'a RuntimeConfig,
    /// Deterministic input pipeline.
    pub input_pipeline: &'a InputPipeline,
    /// Answer policy.
    pub answer_policy: &'a dyn AnswerPolicy,
    /// Optional LLM-backed input normalizer.
    pub llm_normalizer: Option<&'a dyn LlmInputNormalizer>,
    /// Optional server-side memory store.
    pub sessions: Option<&'a dyn SessionMemoryStore>,
    /// Agent transport.
    pub agent: &'a dyn AgentPort,
    /// Audit writer.
    pub audit: &'a AuditWriter,
}

/// Adapter from the existing LLM/MCP loop into runtime frames.
#[derive(Clone)]
pub struct LlmAgentPort {
    base_config: GenerationConfig,
    tools: Arc<Vec<ChatCompletionTool>>,
    mcp: McpHandle,
}

impl LlmAgentPort {
    /// Create an adapter over the existing LLM connector.
    pub fn new(
        base_config: GenerationConfig,
        tools: Arc<Vec<ChatCompletionTool>>,
        mcp: McpHandle,
    ) -> Self {
        Self {
            base_config,
            tools,
            mcp,
        }
    }
}

#[async_trait]
impl AgentPort for LlmAgentPort {
    async fn stream_turn(
        &self,
        input: AgentTurnInput,
    ) -> RuntimeResult<BoxStream<'static, AgentTurnFrame>> {
        let mut cfg = self.base_config.clone();
        cfg.user_prompt = input.prompt;
        cfg.history = input.history;
        let stream = llm_connector::agent_stream(cfg, self.tools.clone(), self.mcp.clone())
            .map(llm_event_to_turn_frame)
            .boxed();
        Ok(stream)
    }
}

/// Run one runtime-owned agent turn.
pub async fn run_agent_turn(
    input: AgentTurnInput,
    audit_ctx: &AuditCtx,
    deps: AgentTurnDeps<'_>,
) -> RuntimeResult<AgentTurnOutcome> {
    let started = Instant::now();
    deps.audit
        .write(
            audit_ctx,
            AuditEvent::RequestReceived {
                input_hash: hash_identifier(&input.prompt),
                input_chars: input.prompt.chars().count(),
                option_id: input.option_id.clone(),
            },
        )
        .await?;

    if let Err(err) = validate_prompt(&input.prompt, deps.runtime_config.input.max_prompt_chars) {
        deps.audit
            .write(
                audit_ctx,
                AuditEvent::InputRejected {
                    code: runtime_error_code(&err).to_string(),
                    reason: err.to_string(),
                },
            )
            .await?;
        return Ok(AgentTurnOutcome::Error {
            code: runtime_error_code(&err).to_string(),
            status: 400,
        });
    }

    let mut normalized = deps.input_pipeline.run_with_config(
        deps.runtime_config,
        &input.prompt,
        input.option_id.as_deref(),
    )?;
    if normalized.confidence < deps.runtime_config.thresholds.confidence.answer_normal {
        if let Some(normalizer) = deps.llm_normalizer {
            normalized = normalizer.normalize(normalized).await?;
        }
    }
    deps.audit
        .write(
            audit_ctx,
            AuditEvent::InputNormalized {
                intent: normalized.intent.clone(),
                confidence: normalized.confidence,
                intent_source: normalized
                    .intent_source
                    .as_ref()
                    .map(|source| format!("{source:?}")),
                warnings: normalized
                    .warnings
                    .iter()
                    .map(|warning| warning.code.clone())
                    .collect(),
            },
        )
        .await?;

    match deps.answer_policy.decide(&normalized) {
        AnswerDecision::Refuse(reason) => {
            let copy = refusal_copy(&reason);
            append_memory_turn_if_enabled(&input, deps.sessions, &normalized, &copy).await?;
            deps.audit
                .write(
                    audit_ctx,
                    AuditEvent::Refused {
                        reason: reason.clone(),
                    },
                )
                .await?;
            deps.audit
                .write(
                    audit_ctx,
                    AuditEvent::ResponseCompleted {
                        response_hash: hash_identifier(&copy),
                        response_chars: copy.chars().count(),
                        duration_ms: started.elapsed().as_millis() as u64,
                        status: "refused".to_string(),
                    },
                )
                .await?;
            Ok(AgentTurnOutcome::Refused { reason, copy })
        }
        AnswerDecision::Disclaimer(disclaimer) => {
            let mut response = disclaimer_copy(&disclaimer);
            let agent_input = apply_memory_context(input, audit_ctx, deps, &normalized).await?;
            stream_agent_response(
                agent_input,
                audit_ctx,
                deps,
                started,
                &normalized,
                &mut response,
            )
            .await
        }
        AnswerDecision::Answer => {
            let mut response = String::new();
            let agent_input = apply_memory_context(input, audit_ctx, deps, &normalized).await?;
            stream_agent_response(
                agent_input,
                audit_ctx,
                deps,
                started,
                &normalized,
                &mut response,
            )
            .await
        }
    }
}

async fn stream_agent_response(
    input: AgentTurnInput,
    audit_ctx: &AuditCtx,
    deps: AgentTurnDeps<'_>,
    started: Instant,
    normalized: &NormalizedInput,
    response: &mut String,
) -> RuntimeResult<AgentTurnOutcome> {
    let memory_input = input.clone();
    let mut stream = deps.agent.stream_turn(input).await?;
    while let Some(frame) = stream.next().await {
        match frame {
            AgentTurnFrame::Token { data } => response.push_str(&data),
            AgentTurnFrame::Clear => {
                response.clear();
                deps.audit
                    .write(audit_ctx, AuditEvent::AnswerCleared)
                    .await?;
            }
            AgentTurnFrame::ToolCalled { name, args_hash } => {
                deps.audit
                    .write(
                        audit_ctx,
                        AuditEvent::ToolCalled {
                            tool: name,
                            args_hash,
                        },
                    )
                    .await?;
            }
            AgentTurnFrame::ToolResult { name, bytes, ok } => {
                deps.audit
                    .write(
                        audit_ctx,
                        AuditEvent::ToolResult {
                            tool: name,
                            bytes,
                            ok,
                        },
                    )
                    .await?;
            }
            AgentTurnFrame::Done => {
                append_memory_turn_if_enabled(&memory_input, deps.sessions, normalized, response)
                    .await?;
                deps.audit
                    .write(
                        audit_ctx,
                        AuditEvent::ResponseCompleted {
                            response_hash: hash_identifier(response),
                            response_chars: response.chars().count(),
                            duration_ms: started.elapsed().as_millis() as u64,
                            status: "completed".to_string(),
                        },
                    )
                    .await?;
                return Ok(AgentTurnOutcome::Final {
                    response: response.clone(),
                    intent: normalized.intent.clone(),
                });
            }
            AgentTurnFrame::Error { data } if response.is_empty() => {
                deps.audit
                    .write(
                        audit_ctx,
                        AuditEvent::ResponseFailed {
                            error_code: data,
                            duration_ms: started.elapsed().as_millis() as u64,
                        },
                    )
                    .await?;
                return Ok(AgentTurnOutcome::Error {
                    code: "upstream_error".to_string(),
                    status: 502,
                });
            }
            AgentTurnFrame::Error { .. } => {
                append_memory_turn_if_enabled(&memory_input, deps.sessions, normalized, response)
                    .await?;
                deps.audit
                    .write(
                        audit_ctx,
                        AuditEvent::ResponseCompleted {
                            response_hash: hash_identifier(response),
                            response_chars: response.chars().count(),
                            duration_ms: started.elapsed().as_millis() as u64,
                            status: "aborted".to_string(),
                        },
                    )
                    .await?;
                return Ok(AgentTurnOutcome::Aborted {
                    response: response.clone(),
                });
            }
        }
    }

    Ok(AgentTurnOutcome::Aborted {
        response: response.clone(),
    })
}

async fn apply_memory_context(
    mut input: AgentTurnInput,
    audit_ctx: &AuditCtx,
    deps: AgentTurnDeps<'_>,
    _normalized: &NormalizedInput,
) -> RuntimeResult<AgentTurnInput> {
    let Some(session_id) = input.session_id.clone() else {
        return Ok(input);
    };
    let Some(sessions) = deps.sessions else {
        return Ok(input);
    };
    let scope = SessionMemoryScope {
        session_id,
        actor_id: None,
    };
    let Some(memory) = sessions.get(&scope).await else {
        deps.audit
            .write(
                audit_ctx,
                AuditEvent::MemoryContext {
                    used_turn_count: 0,
                    dropped_reason: None,
                },
            )
            .await?;
        input.history.clear();
        return Ok(input);
    };
    match build_session_memory_context(
        &memory,
        deps.runtime_config
            .thresholds
            .memory
            .max_memory_context_chars,
    ) {
        Some(context) => {
            deps.audit
                .write(
                    audit_ctx,
                    AuditEvent::MemoryContext {
                        used_turn_count: memory.recent_turns.len(),
                        dropped_reason: None,
                    },
                )
                .await?;
            input.prompt = format!("{context}\n\nCurrent user input:\n{}", input.prompt);
            input.history.clear();
        }
        None => {
            deps.audit
                .write(
                    audit_ctx,
                    AuditEvent::MemoryContext {
                        used_turn_count: 0,
                        dropped_reason: Some("budget_exhausted".into()),
                    },
                )
                .await?;
            input.history.clear();
        }
    }
    Ok(input)
}

async fn append_memory_turn_if_enabled(
    input: &AgentTurnInput,
    sessions: Option<&dyn SessionMemoryStore>,
    normalized: &NormalizedInput,
    response: &str,
) -> RuntimeResult<()> {
    let Some(session_id) = input.session_id.as_ref() else {
        return Ok(());
    };
    let Some(sessions) = sessions else {
        return Ok(());
    };
    let scope = SessionMemoryScope {
        session_id: session_id.clone(),
        actor_id: None,
    };
    sessions
        .append_turn(
            &scope,
            SessionMemoryTurn {
                turn_id: input.request_id.to_string(),
                user_summary: memory_user_summary(&input.prompt),
                answer_summary: response.to_string(),
                intent: Some(normalized.intent.clone()),
                metric: normalized.slots.metric.clone(),
                asset: normalized.slots.asset.clone(),
                time_range_label: normalized.slots.time_range.clone(),
                option_id: input.option_id.clone(),
                created_at_ms: now_ms(),
            },
        )
        .await;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn memory_user_summary(prompt: &str) -> String {
    prompt
        .rsplit_once("Current user input:\n")
        .map(|(_, current)| current.to_string())
        .unwrap_or_else(|| prompt.to_string())
}

fn llm_event_to_turn_frame(event: LlmEvent) -> AgentTurnFrame {
    match event {
        LlmEvent::Token(data) => AgentTurnFrame::Token { data },
        LlmEvent::Clear => AgentTurnFrame::Clear,
        LlmEvent::ToolCalled { name, args_hash } => AgentTurnFrame::ToolCalled { name, args_hash },
        LlmEvent::ToolResult { name, bytes, ok } => AgentTurnFrame::ToolResult { name, bytes, ok },
        LlmEvent::Done => AgentTurnFrame::Done,
        LlmEvent::Error(data) => AgentTurnFrame::Error { data },
    }
}

fn runtime_error_code(err: &RuntimeError) -> &'static str {
    match err {
        RuntimeError::InputRequired => "input_required",
        RuntimeError::InputTooLong(_) => "input_too_long",
        _ => "runtime_error",
    }
}

fn refusal_copy(reason: &str) -> String {
    match reason {
        "prompt_injection" => "這個問題包含我不能遵循的指令，因此無法處理。".to_string(),
        _ => "這個問題超出我目前能回答的範圍。".to_string(),
    }
}

fn disclaimer_copy(reason: &str) -> String {
    match reason {
        "low_confidence" => "（以下為初步判讀，可能需要進一步確認）\n\n".to_string(),
        _ => "（以下回答可能需要進一步確認）\n\n".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures::stream;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    use super::*;
    use crate::config::AppConfig;
    use crate::model::History;
    use crate::runtime::audit::{AuditFailurePolicy, AuditRecord, AuditSink};
    use crate::runtime::guardrails::answer_policy::RuleAnswerPolicy;
    use crate::runtime::memory::store::{
        InMemorySessionStore, SessionMemoryStore, SessionMemoryTurn,
    };
    use crate::runtime::registry::BuiltinRegistry;

    #[test]
    fn maps_llm_events_to_runtime_frames() {
        assert_eq!(
            llm_event_to_turn_frame(LlmEvent::Token("hi".into())),
            AgentTurnFrame::Token { data: "hi".into() }
        );
        assert_eq!(
            llm_event_to_turn_frame(LlmEvent::ToolCalled {
                name: "query".into(),
                args_hash: "abc".into(),
            }),
            AgentTurnFrame::ToolCalled {
                name: "query".into(),
                args_hash: "abc".into(),
            }
        );
        assert_eq!(
            llm_event_to_turn_frame(LlmEvent::ToolResult {
                name: "query".into(),
                bytes: 42,
                ok: true,
            }),
            AgentTurnFrame::ToolResult {
                name: "query".into(),
                bytes: 42,
                ok: true,
            }
        );
        assert_eq!(
            llm_event_to_turn_frame(LlmEvent::Clear),
            AgentTurnFrame::Clear
        );
        assert_eq!(
            llm_event_to_turn_frame(LlmEvent::Done),
            AgentTurnFrame::Done
        );
        assert_eq!(
            llm_event_to_turn_frame(LlmEvent::Error("boom".into())),
            AgentTurnFrame::Error {
                data: "boom".into(),
            }
        );
    }

    #[derive(Clone)]
    struct FakeAgentPort {
        frames: Vec<AgentTurnFrame>,
        calls: Arc<Mutex<usize>>,
        last_input: Arc<Mutex<Option<AgentTurnInput>>>,
    }

    #[async_trait]
    impl AgentPort for FakeAgentPort {
        async fn stream_turn(
            &self,
            input: AgentTurnInput,
        ) -> RuntimeResult<BoxStream<'static, AgentTurnFrame>> {
            *self.calls.lock().await += 1;
            *self.last_input.lock().await = Some(input);
            Ok(stream::iter(self.frames.clone()).boxed())
        }
    }

    #[derive(Debug, Default)]
    struct CapturingAuditSink {
        records: Mutex<Vec<AuditRecord>>,
    }

    #[async_trait]
    impl AuditSink for CapturingAuditSink {
        async fn write(&self, ctx: &AuditCtx, seq: u64, event: AuditEvent) -> RuntimeResult<()> {
            self.records
                .lock()
                .await
                .push(AuditRecord::from_event(ctx, seq, event));
            Ok(())
        }
    }

    struct FakeNormalizer {
        calls: Arc<Mutex<usize>>,
        intent: String,
        confidence: f32,
    }

    #[async_trait]
    impl LlmInputNormalizer for FakeNormalizer {
        async fn normalize(&self, mut input: NormalizedInput) -> RuntimeResult<NormalizedInput> {
            *self.calls.lock().await += 1;
            input.intent = self.intent.clone();
            input.confidence = self.confidence;
            Ok(input)
        }
    }

    fn runtime_config() -> RuntimeConfig {
        let refs = AppConfig::load("config/config.toml")
            .expect("app config should load")
            .runtime
            .expect("runtime refs should exist");
        RuntimeConfig::load(&refs, &BuiltinRegistry::default()).expect("runtime config should load")
    }

    fn turn_input(prompt: &str) -> AgentTurnInput {
        AgentTurnInput {
            request_id: Uuid::nil(),
            prompt: prompt.to_string(),
            history: Vec::new(),
            session_id: None,
            option_id: None,
        }
    }

    fn audit_ctx() -> AuditCtx {
        AuditCtx {
            request_id: "req".into(),
            session_id: None,
            route: "/agent".into(),
            actor: None,
        }
    }

    async fn run_with_fake_agent(
        input: AgentTurnInput,
        frames: Vec<AgentTurnFrame>,
    ) -> (AgentTurnOutcome, Arc<CapturingAuditSink>, Arc<Mutex<usize>>) {
        let cfg = runtime_config();
        let pipeline = InputPipeline::default();
        let policy = RuleAnswerPolicy;
        let calls = Arc::new(Mutex::new(0));
        let agent = FakeAgentPort {
            frames,
            calls: calls.clone(),
            last_input: Arc::new(Mutex::new(None)),
        };
        let audit_sink = Arc::new(CapturingAuditSink::default());
        let audit = AuditWriter::new(audit_sink.clone(), AuditFailurePolicy::FailClosed);
        let outcome = run_agent_turn(
            input,
            &audit_ctx(),
            AgentTurnDeps {
                runtime_config: &cfg,
                input_pipeline: &pipeline,
                answer_policy: &policy,
                llm_normalizer: None,
                sessions: None,
                agent: &agent,
                audit: &audit,
            },
        )
        .await
        .expect("turn should run");
        (outcome, audit_sink, calls)
    }

    async fn run_with_sessions(
        input: AgentTurnInput,
        sessions: Option<&dyn SessionMemoryStore>,
    ) -> (
        AgentTurnOutcome,
        Arc<Mutex<Option<AgentTurnInput>>>,
        Arc<CapturingAuditSink>,
    ) {
        let cfg = runtime_config();
        let pipeline = InputPipeline::default();
        let policy = RuleAnswerPolicy;
        let last_input = Arc::new(Mutex::new(None));
        let agent = FakeAgentPort {
            frames: vec![
                AgentTurnFrame::Token {
                    data: "answer".into(),
                },
                AgentTurnFrame::Done,
            ],
            calls: Arc::new(Mutex::new(0)),
            last_input: last_input.clone(),
        };
        let audit_sink = Arc::new(CapturingAuditSink::default());
        let audit = AuditWriter::new(audit_sink.clone(), AuditFailurePolicy::FailClosed);
        let outcome = run_agent_turn(
            input,
            &audit_ctx(),
            AgentTurnDeps {
                runtime_config: &cfg,
                input_pipeline: &pipeline,
                answer_policy: &policy,
                llm_normalizer: None,
                sessions,
                agent: &agent,
                audit: &audit,
            },
        )
        .await
        .expect("turn should run");
        (outcome, last_input, audit_sink)
    }

    async fn run_with_normalizer(
        input: AgentTurnInput,
        normalizer: &dyn LlmInputNormalizer,
    ) -> (AgentTurnOutcome, Arc<Mutex<usize>>) {
        let cfg = runtime_config();
        let pipeline = InputPipeline::default();
        let policy = RuleAnswerPolicy;
        let calls = Arc::new(Mutex::new(0));
        let agent = FakeAgentPort {
            frames: vec![
                AgentTurnFrame::Token {
                    data: "answer".into(),
                },
                AgentTurnFrame::Done,
            ],
            calls: calls.clone(),
            last_input: Arc::new(Mutex::new(None)),
        };
        let audit_sink = Arc::new(CapturingAuditSink::default());
        let audit = AuditWriter::new(audit_sink, AuditFailurePolicy::FailClosed);
        let outcome = run_agent_turn(
            input,
            &audit_ctx(),
            AgentTurnDeps {
                runtime_config: &cfg,
                input_pipeline: &pipeline,
                answer_policy: &policy,
                llm_normalizer: Some(normalizer),
                sessions: None,
                agent: &agent,
                audit: &audit,
            },
        )
        .await
        .expect("turn should run");
        (outcome, calls)
    }

    #[tokio::test]
    async fn clear_frame_clears_buffer() {
        let (outcome, audit, _) = run_with_fake_agent(
            turn_input("營收 收入 賺多少"),
            vec![
                AgentTurnFrame::Token {
                    data: "preamble".into(),
                },
                AgentTurnFrame::Clear,
                AgentTurnFrame::Token {
                    data: "final".into(),
                },
                AgentTurnFrame::Done,
            ],
        )
        .await;

        assert_eq!(
            outcome,
            AgentTurnOutcome::Final {
                response: "final".into(),
                intent: "revenue".into(),
            }
        );
        assert!(audit
            .records
            .lock()
            .await
            .iter()
            .any(|record| matches!(record.event, AuditEvent::AnswerCleared)));
    }

    #[tokio::test]
    async fn tool_frames_are_audited() {
        let (_, audit, _) = run_with_fake_agent(
            turn_input("營收 收入 賺多少"),
            vec![
                AgentTurnFrame::ToolCalled {
                    name: "query".into(),
                    args_hash: "abc".into(),
                },
                AgentTurnFrame::ToolResult {
                    name: "query".into(),
                    bytes: 123,
                    ok: true,
                },
                AgentTurnFrame::Token {
                    data: "answer".into(),
                },
                AgentTurnFrame::Done,
            ],
        )
        .await;

        let records = audit.records.lock().await;
        assert!(records
            .iter()
            .any(|record| matches!(record.event, AuditEvent::ToolCalled { .. })));
        assert!(records
            .iter()
            .any(|record| matches!(record.event, AuditEvent::ToolResult { .. })));
    }

    #[tokio::test]
    async fn refusal_does_not_call_upstream() {
        let (outcome, _, calls) = run_with_fake_agent(
            turn_input("未知 其他"),
            vec![AgentTurnFrame::Token {
                data: "should not appear".into(),
            }],
        )
        .await;

        assert!(matches!(outcome, AgentTurnOutcome::Refused { .. }));
        assert_eq!(*calls.lock().await, 0);
    }

    #[tokio::test]
    async fn llm_normalizer_not_called_for_high_confidence_input() {
        let normalizer_calls = Arc::new(Mutex::new(0));
        let normalizer = FakeNormalizer {
            calls: normalizer_calls.clone(),
            intent: "unknown".into(),
            confidence: 0.99,
        };

        let (outcome, agent_calls) =
            run_with_normalizer(turn_input("營收 收入 賺多少"), &normalizer).await;

        assert_eq!(*normalizer_calls.lock().await, 0);
        assert_eq!(*agent_calls.lock().await, 1);
        assert!(matches!(
            outcome,
            AgentTurnOutcome::Final {
                intent,
                ..
            } if intent == "revenue"
        ));
    }

    #[tokio::test]
    async fn llm_normalizer_can_recover_low_confidence_before_policy() {
        let normalizer_calls = Arc::new(Mutex::new(0));
        let normalizer = FakeNormalizer {
            calls: normalizer_calls.clone(),
            intent: "revenue".into(),
            confidence: 0.9,
        };

        let (outcome, agent_calls) = run_with_normalizer(turn_input("zzzz"), &normalizer).await;

        assert_eq!(*normalizer_calls.lock().await, 1);
        assert_eq!(*agent_calls.lock().await, 1);
        assert!(matches!(
            outcome,
            AgentTurnOutcome::Final {
                intent,
                ..
            } if intent == "revenue"
        ));
    }

    #[tokio::test]
    async fn rejected_request_is_audited() {
        let (outcome, audit, calls) = run_with_fake_agent(turn_input("   "), Vec::new()).await;

        assert_eq!(
            outcome,
            AgentTurnOutcome::Error {
                code: "input_required".into(),
                status: 400,
            }
        );
        assert_eq!(*calls.lock().await, 0);
        assert!(audit
            .records
            .lock()
            .await
            .iter()
            .any(|record| matches!(record.event, AuditEvent::InputRejected { .. })));
    }

    #[tokio::test]
    async fn disclaimer_is_prepended_before_agent_tokens() {
        let (outcome, _, _) = run_with_fake_agent(
            turn_input("營收 充電"),
            vec![
                AgentTurnFrame::Token {
                    data: "body".into(),
                },
                AgentTurnFrame::Done,
            ],
        )
        .await;

        let AgentTurnOutcome::Final { response, .. } = outcome else {
            panic!("expected final answer");
        };
        assert!(response.starts_with("（以下為初步判讀"));
        assert!(response.ends_with("body"));
    }

    #[tokio::test]
    async fn upstream_error_empty_buffer_fails_but_partial_buffer_aborts() {
        let (failed, _, _) = run_with_fake_agent(
            turn_input("營收 收入 賺多少"),
            vec![AgentTurnFrame::Error {
                data: "upstream down".into(),
            }],
        )
        .await;
        assert_eq!(
            failed,
            AgentTurnOutcome::Error {
                code: "upstream_error".into(),
                status: 502,
            }
        );

        let (aborted, _, _) = run_with_fake_agent(
            turn_input("營收 收入 賺多少"),
            vec![
                AgentTurnFrame::Token {
                    data: "partial".into(),
                },
                AgentTurnFrame::Error {
                    data: "client gone".into(),
                },
            ],
        )
        .await;
        assert_eq!(
            aborted,
            AgentTurnOutcome::Aborted {
                response: "partial".into(),
            }
        );
    }

    #[tokio::test]
    async fn memory_disabled_uses_client_history() {
        let mut input = turn_input("營收 收入 賺多少");
        input.session_id = Some("s1".into());
        input.history = vec![History {
            user_prompt: "previous".into(),
            model_response: "old answer".into(),
        }];

        let (_, last_input, _) = run_with_sessions(input, None).await;
        let sent = last_input
            .lock()
            .await
            .clone()
            .expect("agent input should be captured");

        assert_eq!(sent.history.len(), 1);
        assert_eq!(sent.prompt, "營收 收入 賺多少");
    }

    #[tokio::test]
    async fn memory_enabled_injects_context_and_clears_upstream_history() {
        let store = InMemorySessionStore::new(5);
        let scope = SessionMemoryScope {
            session_id: "s1".into(),
            actor_id: None,
        };
        store
            .append_turn(
                &scope,
                SessionMemoryTurn {
                    turn_id: "prior".into(),
                    user_summary: "上個月營收".into(),
                    answer_summary: "100 元".into(),
                    intent: Some("revenue".into()),
                    metric: Some("revenue".into()),
                    asset: None,
                    time_range_label: None,
                    option_id: None,
                    created_at_ms: 1,
                },
            )
            .await;
        let mut input = turn_input("營收 收入 這個月呢");
        input.session_id = Some("s1".into());
        input.history = vec![History {
            user_prompt: "client history".into(),
            model_response: "should not be sent".into(),
        }];

        let (_, last_input, audit) = run_with_sessions(input, Some(&store)).await;
        let sent = last_input
            .lock()
            .await
            .clone()
            .expect("agent input should be captured");

        assert!(sent.history.is_empty());
        assert!(sent.prompt.contains("Session memory"));
        assert!(sent.prompt.contains("上個月營收"));
        assert!(sent
            .prompt
            .ends_with("Current user input:\n營收 收入 這個月呢"));
        assert!(audit.records.lock().await.iter().any(|record| matches!(
            record.event,
            AuditEvent::MemoryContext {
                used_turn_count: 1,
                dropped_reason: None
            }
        )));
        let memory = store.get(&scope).await.expect("memory should remain");
        assert_eq!(
            memory
                .recent_turns
                .last()
                .map(|turn| turn.user_summary.as_str()),
            Some("營收 收入 這個月呢")
        );
    }
}
