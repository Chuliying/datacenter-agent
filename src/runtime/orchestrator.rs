//! Runtime turn orchestrator.

use std::sync::Arc;
use std::time::Instant;

use async_openai::types::chat::ChatCompletionTool;
use async_trait::async_trait;
use futures::{stream::BoxStream, StreamExt};

use super::audit::{hash_identifier, AuditCtx, AuditEvent, AuditWriter};
use super::config::RuntimeConfig;
use super::error::{RuntimeError, RuntimeResult};
use super::guardrails::answer_policy::{AnswerDecision, AnswerPolicy};
use super::guardrails::input_guard::validate_prompt;
use super::input::pipeline::InputPipeline;
use super::schema::{AgentTurnFrame, AgentTurnInput};
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
pub struct AgentTurnDeps<'a> {
    /// Runtime config.
    pub runtime_config: &'a RuntimeConfig,
    /// Deterministic input pipeline.
    pub input_pipeline: &'a InputPipeline,
    /// Answer policy.
    pub answer_policy: &'a dyn AnswerPolicy,
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

    let normalized = deps.input_pipeline.run_with_config(
        deps.runtime_config,
        &input.prompt,
        input.option_id.as_deref(),
    )?;
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
            stream_agent_response(
                input,
                audit_ctx,
                deps,
                started,
                &normalized.intent,
                &mut response,
            )
            .await
        }
        AnswerDecision::Answer => {
            let mut response = String::new();
            stream_agent_response(
                input,
                audit_ctx,
                deps,
                started,
                &normalized.intent,
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
    intent: &str,
    response: &mut String,
) -> RuntimeResult<AgentTurnOutcome> {
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
                    intent: intent.to_string(),
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
    use crate::runtime::audit::{AuditFailurePolicy, AuditRecord, AuditSink};
    use crate::runtime::guardrails::answer_policy::RuleAnswerPolicy;
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
    }

    #[async_trait]
    impl AgentPort for FakeAgentPort {
        async fn stream_turn(
            &self,
            _input: AgentTurnInput,
        ) -> RuntimeResult<BoxStream<'static, AgentTurnFrame>> {
            *self.calls.lock().await += 1;
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
                agent: &agent,
                audit: &audit,
            },
        )
        .await
        .expect("turn should run");
        (outcome, audit_sink, calls)
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
}
