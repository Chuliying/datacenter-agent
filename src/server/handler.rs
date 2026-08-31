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

//! Request handlers.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::{instrument, warn, Instrument};

use rand::seq::SliceRandom;

use futures::StreamExt;

use super::dto::{
    AgentRequest, GreetingResponse, IntentResolvedData, ReadyBody, ReadyChecks, StageData,
    StagePhase, StreamFrame, ToolArgsData, ToolCallData, UsageData,
};
use super::error::AppError;
use super::identity::IdentityContext;
use super::openai;
use super::AppState;
use crate::agent::clock::{Clock, SystemClock};
use crate::agent::config::PipelineId;
use crate::agent::engine::Orchestrator;
use crate::agent::events::{AgentEvent, ChannelSink, EventSink, StageOutcome};
use crate::agent::payload::{AgentError, AgentPayload, Exchange, InitialPrompt};
use crate::agent::pipeline::{agent_pipeline_id, report_pipeline_id, ss_chat_pipeline_id};
use crate::agent::wiring::{build_insight_pipeline, build_report_pipeline, build_ss_chat_pipeline};
use crate::runtime::audit::{hash_identifier, AuditCtx, AuditEvent, AuditWriter};
use crate::runtime::guardrails::answer_policy::{AlwaysAnswerPolicy, AnswerPolicy};
use crate::runtime::schema::{AgentTurnFrame, AgentTurnInput, NormalizedInput};
use crate::runtime::turn::{
    append_memory_turn_if_enabled, plan_stream_turn, AgentPort, AgentTurnDeps, StreamPlan,
    TurnEvent,
};
use crate::server::authz::{authorize_pipeline, authorize_ss_chat, AuthorizationDecision};

/// SSE keep-alive interval.
///
/// Holds the connection open across long model-side pauses
/// (e.g. before the first token arrives) so upstream proxies don't
/// close the socket on us.
const SSE_KEEPALIVE: Duration = Duration::from_secs(15);

// ──── /health ───

/// Health check endpoint.
pub async fn health() -> StatusCode {
    StatusCode::OK
}

// ──── /ready ───

/// Readiness endpoint.
#[instrument(skip(state))]
pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let api_key = !state.llm.api_key.is_empty();

    let base_url_reachable = match state.http.head(&state.llm.base_url).send().await {
        Ok(_) => true,
        Err(e) => {
            warn!(error = %e, base_url = %state.llm.base_url, "ready: base url probe failed");
            false
        }
    };

    let ready = api_key && base_url_reachable;
    let body = ReadyBody {
        ready,
        checks: ReadyChecks {
            api_key,
            base_url_reachable,
        },
    };

    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status, Json(body))
}

// ──── /greeting ───

#[instrument(skip(state))]
pub async fn greeting(State(state): State<AppState>) -> Result<Json<GreetingResponse>, AppError> {
    let picked = {
        let v = state.greetings.lock().await;
        v.choose(&mut rand::thread_rng()).cloned()
    };
    match picked {
        Some(greeting) => Ok(Json(GreetingResponse { greeting })),
        None => Err(AppError::ServiceUnavailable(
            "greeting not ready, retry shortly".into(),
        )),
    }
}

// ──── /agent/stream ───

/// Intent id that routes a turn to the report pipeline (defined in
/// `config/runtime/intents.toml`).
const REPORT_INTENT: &str = "report";

/// Route to the report pipeline when the user asked for a report — the `report`
/// intent was *mentioned* (the resolved top intent, or present among the
/// candidates). Otherwise the insight pipeline.
///
/// Keying off "mentioned" rather than strict top-1 means a topic-plus-report
/// prompt like `營收報告` still routes to the report while its topic intent
/// (`revenue`) keeps driving answer policy + memory.
fn wants_report_pipeline(normalized: &NormalizedInput) -> bool {
    normalized.intent == REPORT_INTENT
        || normalized
            .candidate_intents
            .iter()
            .any(|candidate| candidate == REPORT_INTENT)
}

fn insufficient_permission_copy(decision: &AuthorizationDecision) -> String {
    let missing = if decision.omitted_topics.is_empty() {
        "此主題"
    } else {
        &decision.omitted_topics.join("、")
    };
    format!("權限不足，無法提供{missing}資料。請向管理者申請相應權限。")
}

fn permission_degradation_prefix(decision: &AuthorizationDecision) -> String {
    if decision.omitted_topics.is_empty() {
        String::new()
    } else {
        format!(
            "本報告因權限不足，省略以下主題：{}。",
            decision.omitted_topics.join("、")
        )
    }
}

/// Map the runtime's HTTP-ish status onto the host error contract for a
/// pre-stream [`StreamPlan::Error`] (e.g. an invalid prompt).
fn status_to_app_error(status: u16, code: String) -> AppError {
    match status {
        400..=499 => AppError::BadRequest(code),
        _ => AppError::ServiceUnavailable(code),
    }
}

/// A no-op [`AgentPort`] to satisfy [`AgentTurnDeps`]. [`plan_stream_turn`] runs
/// only the synchronous prelude and never touches the agent transport, but the
/// shared deps struct requires one; `/agent/stream` drives the sub-agent pipeline
/// itself rather than through this port.
struct UnusedAgentPort;

#[async_trait::async_trait]
impl AgentPort for UnusedAgentPort {
    async fn stream_turn(
        &self,
        _input: AgentTurnInput,
    ) -> crate::runtime::error::RuntimeResult<futures::stream::BoxStream<'static, AgentTurnFrame>>
    {
        Ok(futures::stream::empty().boxed())
    }
}

/// Server-Sent Events analytics front door that runs the **full runtime turn**
/// (guardrails → intent → memory → audit) and then routes to the sub-agent
/// pipeline the resolved intent selects: the report pipeline when a report was
/// asked for (see [`wants_report_pipeline`]), else the insight pipeline.
///
/// This is the only native streaming front door for the EV-charging (EOMC) tools. The retired
/// `insight_stream` / `report_stream` handlers drove one fixed pipeline directly and bypassed the
/// runtime; this one reuses the runtime's [`plan_stream_turn`] prelude verbatim (no
/// duplicated guardrail/intent logic), then streams the chosen pipeline's rich stage
/// frames through the `insight_frames` mapping — and replicates the runtime turn's
/// two post-stream side effects
/// (`ResponseCompleted` / `ResponseFailed` audit + session-memory append).
///
/// The streaming body is shared with [`ss_chat_stream`] through [`run_chat_stream`]; this handler
/// contributes the route's own two decisions: the runtime's configured answer policy (so
/// off-scope prompts are refused), and intent-driven pipeline selection.
///
/// Requires the runtime to be enabled (`RUNTIME_ENABLED`, default on); rolled back,
/// this returns `503`. There is no alternative path: the forced-pipeline endpoints that used to
/// serve as a rollback target were retired (they were also the only prompt entry points that
/// bypassed the prelude), so a rolled-back runtime leaves only `/health`, `/ready` and
/// `/greeting` available.
pub async fn agent_stream(
    State(state): State<AppState>,
    Extension(identity): Extension<IdentityContext>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let Json(req) = req?;
    let span = tracing::info_span!(
        "agent-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
        session_id = req.session_id.as_deref().unwrap_or(""),
        option_id = req.option_id.as_deref().unwrap_or(""),
    );
    run_chat_stream(
        state,
        identity,
        req,
        "/agent/stream",
        None,
        authorize_agent_route,
        select_agent_pipeline,
    )
    .instrument(span)
    .await
}

/// Server-Sent Events front door for the **星星電力 (SS) investor platform**: the same four-stage
/// chat pipeline as [`agent_stream`]'s insight path, over the six `ss_*` tools.
///
/// Identical in wire contract to `/agent/stream` — same [`AgentRequest`] body, same
/// [`StreamFrame`] SSE frames, same bearer gate, same opt-in burst limiter — and it runs the same
/// [`plan_stream_turn`] prelude, so prompt-length validation, the injection guardrail, session
/// memory and the audit trail all apply unchanged. Two things differ:
///
/// - **No intent filtering.** The runtime's intent pack (`config/runtime/intents.toml`) describes
///   the EV-charging domain, so every SS question resolves to `unknown` and the configured
///   [`RuleAnswerPolicy`](crate::runtime::guardrails::answer_policy::RuleAnswerPolicy) would refuse
///   it as `off_scope` before the pipeline ever ran. This route substitutes
///   [`AlwaysAnswerPolicy`], which keeps the prompt-injection refusal but drops the scope gate.
///   Intent still resolves and is still emitted as `intent.resolved` / audited — it just no longer
///   decides anything here.
/// - **No pipeline routing.** There is one SS pipeline today; a report counterpart is a later
///   step, so [`wants_report_pipeline`] is deliberately not consulted.
///
/// Requires the runtime to be enabled (`RUNTIME_ENABLED`, default on); rolled back, returns `503`.
pub async fn ss_chat_stream(
    State(state): State<AppState>,
    Extension(identity): Extension<IdentityContext>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let Json(req) = req?;
    let span = tracing::info_span!(
        "ss-chat-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
        session_id = req.session_id.as_deref().unwrap_or(""),
        option_id = req.option_id.as_deref().unwrap_or(""),
    );
    run_chat_stream(
        state,
        identity,
        req,
        "/ss-chat/stream",
        // Intent filtering off: see the doc comment above.
        Some(&AlwaysAnswerPolicy),
        authorize_ss_chat_route,
        select_ss_chat_pipeline,
    )
    .instrument(span)
    .await
}

/// How a streaming front door turns a resolved turn into a runnable pipeline.
///
/// Taken as a plain `fn` pointer rather than a closure so each route's choice is a named,
/// separately-testable function ([`select_agent_pipeline`] / [`select_ss_chat_pipeline`]) instead
/// of an anonymous body buried in the handler.
type PipelineSelector = fn(
    &AppState,
    &NormalizedInput,
    &AuthorizationDecision,
    Arc<dyn EventSink>,
) -> Result<(Orchestrator, PipelineId), AppError>;

/// Per-route authorization gate, run after the prelude resolves the intent and before any
/// pipeline (and therefore any LLM or MCP call) is built. Both prompt routes carry one; a route
/// without a gate would be an end-user-unauthenticated way into the same pipelines.
type RouteAuthz = fn(&AppState, &IdentityContext, &NormalizedInput) -> AuthorizationDecision;

/// `/agent/stream`'s gate: the boot ∩ permission ∩ intent-required three-way intersection,
/// with the report predicate keyed off [`wants_report_pipeline`] (top **or** candidate intent).
fn authorize_agent_route(
    state: &AppState,
    identity: &IdentityContext,
    normalized: &NormalizedInput,
) -> AuthorizationDecision {
    let advertised = state
        .tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect::<Vec<_>>();
    authorize_pipeline(
        &state.authz,
        if wants_report_pipeline(normalized) {
            &state.report_grants.fetcher
        } else {
            &state.insight_grants.fetcher
        },
        &identity.permissions.codes,
        &normalized.intent,
        wants_report_pipeline(normalized),
        &advertised,
        &state.insight_grants.charter,
        &["emit_report".to_string()],
    )
}

/// `/ss-chat/stream`'s gate. Intent-based gating cannot apply here — the route deliberately
/// disables intent filtering because every SS question resolves to `unknown` under the
/// EV-charging intent pack, and `unknown` maps to no required tools. Authorization instead keys
/// off `[authz].ss_chat_permissions`: holding **any** of those Falcon codes unlocks the full
/// `[ss_chat.grants]` fetcher set; holding none refuses before any LLM or MCP call, exactly like
/// the agent route's empty-intersection refusal.
fn authorize_ss_chat_route(
    state: &AppState,
    identity: &IdentityContext,
    _normalized: &NormalizedInput,
) -> AuthorizationDecision {
    let advertised = state
        .tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect::<Vec<_>>();
    authorize_ss_chat(
        &state.authz,
        &state.ss_chat_grants.fetcher,
        &identity.permissions.codes,
        &advertised,
        &state.ss_chat_grants.charter,
    )
}

/// `/agent/stream`'s selector: the report pipeline when a report was asked for (see
/// [`wants_report_pipeline`]), else the insight pipeline.
fn select_agent_pipeline(
    state: &AppState,
    normalized: &NormalizedInput,
    decision: &AuthorizationDecision,
    sink: Arc<dyn EventSink>,
) -> Result<(Orchestrator, PipelineId), AppError> {
    let resolved = state.llm.resolved();
    if wants_report_pipeline(normalized) {
        let orch = build_report_pipeline(
            state.mcp.clone(),
            &state.tools,
            state.instructions.as_deref(),
            &state.prompts.fetcher_system,
            &state.prompts.report_analyst_system,
            &state.prompts.report_composer_system,
            &decision.effective_data_grant,
            &resolved,
            state.report_template.clone(),
            Some(sink),
        )
        .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
        Ok((orch, report_pipeline_id()))
    } else {
        let orch = build_insight_pipeline(
            state.mcp.clone(),
            &state.tools,
            state.instructions.as_deref(),
            &state.prompts.fetcher_system,
            &state.prompts.analyst_system,
            &state.prompts.charter_system,
            &decision.effective_data_grant,
            &state.insight_grants.charter,
            &resolved,
            Some(sink),
        )
        .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
        Ok((orch, agent_pipeline_id()))
    }
}

/// `/ss-chat/stream`'s selector: always the SS chat pipeline.
///
/// `normalized` is unused on purpose — this route does no intent routing (the SS report pipeline
/// is a later step). Keeping the shared [`PipelineSelector`] shape means adding one later is a
/// change here, not in the streaming body.
fn select_ss_chat_pipeline(
    state: &AppState,
    _normalized: &NormalizedInput,
    decision: &AuthorizationDecision,
    sink: Arc<dyn EventSink>,
) -> Result<(Orchestrator, PipelineId), AppError> {
    let orch = build_ss_chat_pipeline(
        state.mcp.clone(),
        &state.tools,
        state.instructions.as_deref(),
        &state.prompts.ss_fetcher_system,
        &state.prompts.ss_analyst_system,
        &state.prompts.ss_charter_system,
        &decision.effective_data_grant,
        &state.ss_chat_grants.charter,
        &state.llm.resolved(),
        Some(sink),
    )
    .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
    Ok((orch, ss_chat_pipeline_id()))
}

/// The streaming body shared by every SSE chat front door ([`agent_stream`], [`ss_chat_stream`]).
///
/// Runs the runtime prelude, acts on the resulting [`StreamPlan`], drives `select_pipeline`'s
/// orchestrator on a [`ChannelSink`], maps its events into [`StreamFrame`]s, and replicates the
/// runtime turn's two post-stream side effects (audit + session-memory append). A route
/// contributes exactly three things:
///
/// - `route` — the audit `route` label, and the prefix on this handler's own warnings;
/// - `answer_policy` — `None` uses the runtime's configured policy; `Some` overrides it (that is
///   how `/ss-chat/stream` turns intent filtering off without touching the shared prelude);
/// - `select_pipeline` — which pipeline the resolved turn runs.
#[allow(clippy::too_many_arguments)]
async fn run_chat_stream(
    state: AppState,
    identity: IdentityContext,
    req: AgentRequest,
    route: &'static str,
    answer_policy: Option<&(dyn AnswerPolicy + 'static)>,
    route_authz: RouteAuthz,
    select_pipeline: PipelineSelector,
) -> Result<Response, AppError> {
    let runtime = state
        .runtime
        .clone()
        .filter(|rt| rt.enabled)
        .ok_or_else(|| {
            AppError::ServiceUnavailable(format!(
                "runtime disabled (RUNTIME_ENABLED=false); {route} requires the runtime"
            ))
        })?;

    // ── runtime prelude: audit, guardrails, intent, answer policy, memory ──
    // Reuses the runtime turn's synchronous prelude verbatim. The dummy port +
    // no-op emit are never exercised by `plan_stream_turn`; this handler owns
    // the streaming itself.
    let request_id = uuid::Uuid::new_v4();
    let audit_ctx = AuditCtx {
        request_id: request_id.to_string(),
        session_id: req.session_id.clone(),
        route: route.into(),
        actor_key: Some(identity.actor_key.as_str().to_string()),
        actor: None,
    };
    let input = AgentTurnInput {
        request_id,
        prompt: req.prompt.clone(),
        raw_input: req.prompt.clone(),
        // Identity-protected requests never trust client-supplied conversation history:
        // server-side session memory is the only context source (FR-009).
        history: Vec::new(),
        session_id: req.session_id.clone(),
        option_id: req.option_id.clone(),
        identity: Some(identity.clone()),
    };
    let audit = AuditWriter::new(runtime.audit_sink.clone(), runtime.audit_failure_policy);

    let advertised = state
        .tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect::<Vec<_>>();
    let plan = {
        let unused = UnusedAgentPort;
        let emit_noop = |_event: TurnEvent| {};
        let deps = AgentTurnDeps {
            runtime_config: &runtime.config,
            input_pipeline: &runtime.input_pipeline,
            // The route's override, else the runtime's configured policy.
            answer_policy: answer_policy.unwrap_or(runtime.answer_policy.as_ref()),
            llm_normalizer: runtime.llm_normalizer.as_deref(),
            sessions: runtime.sessions.as_deref(),
            agent: &unused,
            audit: &audit,
            emit: &emit_noop,
            authz: Some(&state.authz),
            advertised_tools: &advertised,
            insight_grant: &state.insight_grants.fetcher,
            report_grant: &state.report_grants.fetcher,
        };
        plan_stream_turn(input, &audit_ctx, deps)
            .await
            .map_err(|e| AppError::ServiceUnavailable(format!("runtime prelude: {e}")))?
    };

    // ── act on the plan ──
    let (started, prefix, agent_input, normalized) = match plan {
        StreamPlan::Error { code, status } => {
            // Pre-stream validation error; audit already recorded it.
            return Err(status_to_app_error(status, code));
        }
        StreamPlan::Refused { copy, .. } => {
            // Guardrail refusal: audit + memory already written. Stream the
            // refusal copy as the whole answer, then close.
            let sse = async_stream::stream! {
                yield Ok::<_, Infallible>(sse_event(StreamFrame::Token { data: copy }));
                yield Ok::<_, Infallible>(sse_event(StreamFrame::Done));
            };
            return Ok(Sse::new(sse)
                .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
                .into_response());
        }
        StreamPlan::Proceed {
            started,
            prefix,
            agent_input,
            normalized,
        } => (started, prefix, *agent_input, *normalized),
    };

    // ── authorization gate: after intent resolution, before any pipeline/LLM/MCP work ──
    let decision = route_authz(&state, &identity, &normalized);
    if !decision.allowed {
        let copy = insufficient_permission_copy(&decision);
        if let Err(err) = audit
            .write(
                &audit_ctx,
                AuditEvent::ResponseFailed {
                    error_code: crate::server::codes::AUTHZ_INSUFFICIENT.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            )
            .await
        {
            warn!(error = %err, %route, "chat stream: audit authorization refusal failed");
        }
        let sse = async_stream::stream! {
            yield Ok::<_, Infallible>(sse_event(StreamFrame::Token { data: copy }));
            yield Ok::<_, Infallible>(sse_event(StreamFrame::Refusal {
                code: crate::server::codes::AUTHZ_INSUFFICIENT.to_string(),
            }));
            yield Ok::<_, Infallible>(sse_event(StreamFrame::Done));
        };
        return Ok(Sse::new(sse)
            .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
            .into_response());
    }
    let degradation_prefix = permission_degradation_prefix(&decision);
    if !degradation_prefix.is_empty() {
        audit
            .write(
                &audit_ctx,
                AuditEvent::PermissionDegraded {
                    omitted_topics: decision.omitted_topics.clone(),
                },
            )
            .await
            .map_err(|err| AppError::ServiceUnavailable(format!("audit authorization: {err}")))?;
    }

    // ── build the route's pipeline (streaming) ──
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(INSIGHT_STREAM_BUFFER);
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));
    let (orchestrator, pipeline_id) =
        select_pipeline(&state, &normalized, &decision, sink.clone())?;
    // The pipeline that actually ran, not the top intent: the memory replay filter keys its
    // stricter report rule off this (a mixed-topic report can be stored under a topic intent).
    let ran_report_pipeline = pipeline_id == report_pipeline_id();

    // The memory-augmented prompt/history drive the pipeline; the full
    // agent_input (raw_input preserved) is kept for the post-stream memory append.
    let memory_input = agent_input.clone();
    let initial = AgentPayload::Initial(InitialPrompt {
        prompt: agent_input.prompt,
        history: agent_input
            .history
            .into_iter()
            .map(|turn| Exchange {
                user: turn.user_prompt,
                assistant: turn.model_response,
            })
            .collect(),
        now: SystemClock::default().now(),
    });

    let run = tokio::spawn(async move {
        orchestrator
            .run_emitting(&pipeline_id, initial, &*sink)
            .await
    });

    // State moved into the stream for the intent frame + post-stream side effects.
    let intent = normalized.intent.clone();
    let candidate_intents = normalized.candidate_intents.clone();
    let sessions = runtime.sessions.clone();

    let sse_stream = async_stream::stream! {
        // `intent.resolved` first, mirroring the runtime turn's ordering.
        yield Ok::<_, Infallible>(sse_event(StreamFrame::IntentResolved {
            data: IntentResolvedData { intent, candidate_intents },
        }));
        // A disclaimer prefix, if the answer policy asked for one (transient — the
        // pipeline's terminal `clear` supersedes it, exactly as the runtime turn does).
        if !prefix.is_empty() {
            yield Ok::<_, Infallible>(sse_event(StreamFrame::Token { data: prefix }));
        }

        let mut response = String::new();
        let mut failure: Option<String> = None;
        let mut completed = false;

        while let Some(event) = rx.recv().await {
            match &event {
                AgentEvent::ContentDelta { text } => response.push_str(text),
                // The finalizer/renderer answer is the complete result; the
                // terminal `clear` in `insight_frames` resets the client preview.
                AgentEvent::Finished { assistant } => {
                    response = if degradation_prefix.is_empty() {
                        assistant.clone()
                    } else {
                        with_prefix(&degradation_prefix, assistant.clone())
                    };
                    completed = true;
                }
                AgentEvent::Error { message } => failure = Some(message.clone()),
                _ => {}
            }
            // The degradation notice must survive the terminal `clear`, so it rides on the
            // final answer itself rather than on a transient token (AC-009).
            let event = match event {
                AgentEvent::Finished { assistant } if !degradation_prefix.is_empty() => {
                    AgentEvent::Finished {
                        assistant: with_prefix(&degradation_prefix, assistant),
                    }
                }
                other => other,
            };
            for frame in insight_frames(event) {
                yield Ok::<_, Infallible>(sse_event(frame));
            }
        }

        // Channel closed → the run finished. A stage failure already surfaced as
        // an `error` frame during draining; only a task panic needs a fallback.
        if let Err(join) = run.await {
            yield Ok::<_, Infallible>(sse_event(StreamFrame::Error {
                data: format!("agent task failed: {join}"),
                code: crate::server::codes::SERVER_INTERNAL.to_string(),
            }));
        }

        // ── post-stream side effects (parity with the runtime turn) ──
        let duration_ms = started.elapsed().as_millis() as u64;
        if let Some(error_code) = failure {
            if let Err(e) = audit
                .write(&audit_ctx, AuditEvent::ResponseFailed { error_code, duration_ms })
                .await
            {
                warn!(error = %e, %route, "chat stream: audit ResponseFailed failed");
            }
        } else if completed {
            if let Err(e) = append_memory_turn_if_enabled(
                &memory_input,
                sessions.as_deref(),
                &normalized,
                &response,
                ran_report_pipeline,
            )
            .await
            {
                warn!(error = %e, %route, "chat stream: memory append failed");
            }
            if let Err(e) = audit
                .write(
                    &audit_ctx,
                    AuditEvent::ResponseCompleted {
                        response_hash: hash_identifier(&response),
                        response_chars: response.chars().count(),
                        duration_ms,
                        status: "completed".to_string(),
                    },
                )
                .await
            {
                warn!(error = %e, %route, "chat stream: audit ResponseCompleted failed");
            }
        }
        // else: aborted (no Finished/Error) — mirror `stream_agent_response` (no extra audit).
    };

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
        .into_response())
}

/// Serialize a stream frame into an SSE event.
fn sse_event(frame: StreamFrame) -> Event {
    Event::default()
        .json_data(&frame)
        .expect("unexpected error: StreamFrame is always valid JSON")
}

// ──── /v1/chat/completions (OpenAI-compatible, agentgateway Path C) ────

/// Build an OpenAI-style error [`Response`] with the given HTTP status.
///
/// Unlike [`AppError`] (which serializes to the host's flat `{"error": "..."}`), this emits the
/// OpenAI envelope `{"error": {"message", "type"}}` an OpenAI client / agentgateway expects.
fn openai_error(status: StatusCode, error_type: &str, message: impl Into<String>) -> Response {
    (
        status,
        Json(openai::OpenAiErrorBody::new(error_type, message)),
    )
        .into_response()
}

/// Serialize one `chat.completion.chunk` into an SSE `data:` line — a pure `data:` stream with no
/// `event:` name, as OpenAI expects (spec D1).
fn openai_chunk_event(chunk: &openai::ChatCompletionChunk) -> Event {
    Event::default()
        .json_data(chunk)
        .expect("chat.completion.chunk is always valid JSON")
}

/// Map a `Json` extractor rejection's own HTTP status onto the status this endpoint returns.
///
/// A body over the [`REQUEST_BODY_LIMIT`](super::route) stays `413 Payload Too Large`; a missing or
/// wrong `Content-Type` stays `415 Unsupported Media Type`; every other malformed body (JSON syntax
/// error, or valid JSON of the wrong shape) collapses to `400 Bad Request`. All three are returned
/// with the OpenAI error envelope by the caller.
fn json_rejection_status(rejection_status: StatusCode) -> StatusCode {
    match rejection_status {
        StatusCode::PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
        StatusCode::UNSUPPORTED_MEDIA_TYPE => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// Prepend the answer-policy disclaimer `prefix` to the delivered answer, if any.
///
/// [`StreamPlan::Proceed`] carries a `prefix` (e.g. a "this is not financial advice" disclaimer).
/// Unlike `/agent/stream` — where it streams as a transient token the pipeline's terminal `clear`
/// then supersedes — OpenAI's `delta`/`message` have no retract semantics, so the disclaimer would
/// simply be lost. Prepend it to the final answer instead, so the client actually sees it.
fn with_prefix(prefix: &str, answer: String) -> String {
    if prefix.is_empty() {
        answer
    } else {
        format!("{prefix}\n\n{answer}")
    }
}

/// Resolve the pipeline task's own return value into the authoritative answer / failure.
///
/// The answer and any failure come from the awaited `run_emitting` **result**, never from draining
/// the lossy [`ChannelSink`] (whose `try_send` may silently drop a `Finished` / `Error` event on a
/// full buffer, and which the unknown-pipeline early return never emits at all). Only `Usage` events
/// are read off the channel, where a dropped event merely under-counts tokens.
///
/// - a `Final` payload → its `assistant` text is the answer;
/// - any other terminal payload → a wiring fault (the pipeline ended without a final result);
/// - an [`AgentError`] → its `Display` is the failure message.
fn resolve_outcome(outcome: Result<AgentPayload, AgentError>) -> Result<String, String> {
    match outcome {
        Ok(AgentPayload::Final(result)) => Ok(result.assistant),
        Ok(other) => Err(format!(
            "pipeline produced no final result (got {:?})",
            other.kind()
        )),
        Err(err) => Err(err.to_string()),
    }
}

/// Build the intent-selected sub-agent pipeline for the OpenAI endpoint (spec Data Flow): the
/// report pipeline when a report was asked for (see [`wants_report_pipeline`]), else the insight
/// pipeline.
/// `sink = Some(_)` selects the streaming shape; `None` is buffered (spec D1/D2).
fn build_openai_pipeline(
    state: &AppState,
    report: bool,
    effective_data_grant: &[String],
    sink: Option<Arc<dyn EventSink>>,
) -> anyhow::Result<(Orchestrator, PipelineId)> {
    let resolved = state.llm.resolved();
    if report {
        let orch = build_report_pipeline(
            state.mcp.clone(),
            &state.tools,
            state.instructions.as_deref(),
            &state.prompts.fetcher_system,
            &state.prompts.report_analyst_system,
            &state.prompts.report_composer_system,
            effective_data_grant,
            &resolved,
            state.report_template.clone(),
            sink,
        )?;
        Ok((orch, report_pipeline_id()))
    } else {
        let orch = build_insight_pipeline(
            state.mcp.clone(),
            &state.tools,
            state.instructions.as_deref(),
            &state.prompts.fetcher_system,
            &state.prompts.analyst_system,
            &state.prompts.charter_system,
            effective_data_grant,
            &state.insight_grants.charter,
            &resolved,
            sink,
        )?;
        Ok((orch, agent_pipeline_id()))
    }
}

/// OpenAI-compatible `POST /v1/chat/completions`.
///
/// Maps the OpenAI `messages` onto the internal [`AgentRequest`], runs the **same runtime prelude**
/// as [`agent_stream`] (`plan_stream_turn`: guardrails → intent → answer policy), then drives the
/// intent-selected sub-agent pipeline and shapes the result as OpenAI:
///
/// - `stream=false` (D2): buffered pipeline `run()` → a single `chat.completion` choice.
/// - `stream=true` (D1, pseudo-streaming): the pipeline's **complete** terminal answer is split
///   into `chat.completion.chunk`s, then `data: [DONE]`. There is no token stream equal to the
///   final answer — the terminal pipeline stages assemble it in pure logic — so intermediate
///   `ContentDelta` previews are not forwarded (see the spec's D1).
///
/// The answer / failure are taken from the pipeline task's awaited result (authoritative), not from
/// draining the lossy event channel (finding #3); the answer-policy disclaimer, when the policy
/// asks for one, is prepended to it (finding #7). Auth is the dedicated `require_bearer_openai`
/// layer (finding #6, `401` + OpenAI envelope on a bad token, not the host `418`); the runtime is
/// required (D7, `503` when `RUNTIME_ENABLED=false`). Errors use the OpenAI envelope (spec Errors).
/// `session_id` / `option_id` have no OpenAI equivalent, so server-side memory is inert.
#[instrument(skip_all, fields(route = "/v1/chat/completions"))]
pub async fn chat_completions(
    State(state): State<AppState>,
    Extension(identity): Extension<IdentityContext>,
    req: Result<Json<openai::ChatCompletionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // ── parse body (ERR2: malformed JSON / missing fields) ──
    let Json(req) = match req {
        Ok(json) => json,
        Err(rejection) => {
            // Split by the extractor's own status: body over the limit → 413, missing/wrong
            // content-type → 415, every other malformed body → 400 — all as the OpenAI envelope
            // (finding #6).
            let status = json_rejection_status(rejection.status());
            return openai_error(
                status,
                openai::error_type_for_status(status.as_u16()),
                format!("invalid request body: {rejection}"),
            );
        }
    };

    // ── runtime required (ERR1 / D7): the endpoint drives the runtime prelude ──
    let Some(runtime) = state.runtime.clone().filter(|rt| rt.enabled) else {
        return openai_error(
            StatusCode::SERVICE_UNAVAILABLE,
            openai::ERR_SERVER,
            "runtime disabled (RUNTIME_ENABLED=false); /v1/chat/completions requires the runtime",
        );
    };

    let stream = req.stream;
    let model = req.model;
    // OpenAI `stream_options.include_usage`: append a terminal usage-only chunk to the stream.
    let include_usage = req
        .stream_options
        .map(|opts| opts.include_usage)
        .unwrap_or(false);

    // ── map messages → AgentRequest (ERR2; D5: system messages ignored) ──
    let agent_req = match openai::map_request(req.messages) {
        Ok(request) => request,
        Err(err) => {
            let (status, error_type, message) = err.to_openai();
            return openai_error(
                StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
                error_type,
                message,
            );
        }
    };

    // `created` + `id` are stamped once here (never in the pure `openai` layer) and shared by every
    // response shape.
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default();
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4().simple());

    // ── runtime prelude, shared verbatim with /agent/stream: audit, guardrails, intent, answer
    //    policy. The dummy port + no-op emit are never exercised by `plan_stream_turn`. ──
    let request_id = uuid::Uuid::new_v4();
    let audit_ctx = AuditCtx {
        request_id: request_id.to_string(),
        session_id: agent_req.session_id.clone(),
        route: "/v1/chat/completions".into(),
        actor_key: Some(identity.actor_key.as_str().to_string()),
        actor: None,
    };
    let input = AgentTurnInput {
        request_id,
        // Authorization mode is single-turn: `map_request` already discarded every message
        // except the last user message, and no client history is accepted here.
        prompt: agent_req.prompt.clone(),
        raw_input: agent_req.prompt.clone(),
        history: Vec::new(),
        session_id: agent_req.session_id.clone(),
        option_id: agent_req.option_id.clone(),
        identity: Some(identity.clone()),
    };
    let audit = AuditWriter::new(runtime.audit_sink.clone(), runtime.audit_failure_policy);

    let plan = {
        let unused = UnusedAgentPort;
        let emit_noop = |_event: TurnEvent| {};
        let advertised = state
            .tools
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect::<Vec<_>>();
        let deps = AgentTurnDeps {
            runtime_config: &runtime.config,
            input_pipeline: &runtime.input_pipeline,
            answer_policy: runtime.answer_policy.as_ref(),
            llm_normalizer: runtime.llm_normalizer.as_deref(),
            sessions: runtime.sessions.as_deref(),
            agent: &unused,
            audit: &audit,
            emit: &emit_noop,
            authz: Some(&state.authz),
            advertised_tools: &advertised,
            insight_grant: &state.insight_grants.fetcher,
            report_grant: &state.report_grants.fetcher,
        };
        match plan_stream_turn(input, &audit_ctx, deps).await {
            Ok(plan) => plan,
            Err(e) => {
                return openai_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    openai::ERR_SERVER,
                    format!("runtime prelude: {e}"),
                );
            }
        }
    };

    // ── act on the plan ──
    let (started, prefix, agent_input, normalized) = match plan {
        // Pre-stream validation error (e.g. ERR3 prompt too long → 400). Audit already recorded it.
        StreamPlan::Error { code, status } => {
            return openai_error(
                StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
                openai::error_type_for_status(status),
                code,
            );
        }
        // Guardrail refusal (AC-5 / TC-I04): return the refusal copy as the assistant answer at
        // `200` — governance parity with /agent/stream (which streams the copy then closes).
        StreamPlan::Refused { copy, .. } => {
            return openai_refusal(stream, copy, include_usage, &id, &model, created, None);
        }
        // The answer-policy disclaimer `prefix` (finding #7) is prepended to the delivered answer
        // below — OpenAI's `delta`/`message` have no retract semantics, so unlike /agent/stream
        // (whose terminal `clear` supersedes it) it must ride on the answer itself. `started` is the
        // turn's start instant (from the prelude), carried forward for the post-response audit's
        // `duration_ms` (parity with `agent_stream`).
        StreamPlan::Proceed {
            started,
            prefix,
            agent_input,
            normalized,
        } => (started, prefix, *agent_input, *normalized),
    };

    let advertised = state
        .tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect::<Vec<_>>();
    let decision = authorize_pipeline(
        &state.authz,
        if wants_report_pipeline(&normalized) {
            &state.report_grants.fetcher
        } else {
            &state.insight_grants.fetcher
        },
        &identity.permissions.codes,
        &normalized.intent,
        wants_report_pipeline(&normalized),
        &advertised,
        &state.insight_grants.charter,
        &["emit_report".to_string()],
    );
    if !decision.allowed {
        let copy = insufficient_permission_copy(&decision);
        if let Err(err) = audit
            .write(
                &audit_ctx,
                crate::runtime::audit::AuditEvent::ResponseFailed {
                    error_code: crate::server::codes::AUTHZ_INSUFFICIENT.to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            )
            .await
        {
            warn!(error = %err, "chat_completions: audit authorization refusal failed");
        }
        return openai_refusal(
            stream,
            copy,
            include_usage,
            &id,
            &model,
            created,
            Some(crate::server::codes::AUTHZ_INSUFFICIENT),
        );
    }
    let degradation_prefix = permission_degradation_prefix(&decision);
    if !degradation_prefix.is_empty() {
        if let Err(err) = audit
            .write(
                &audit_ctx,
                crate::runtime::audit::AuditEvent::PermissionDegraded {
                    omitted_topics: decision.omitted_topics.clone(),
                },
            )
            .await
        {
            return openai_error(
                StatusCode::SERVICE_UNAVAILABLE,
                openai::ERR_SERVER,
                format!("audit authorization: {err}"),
            );
        }
    }

    // ── build + run the intent-selected pipeline ──
    let initial = AgentPayload::Initial(InitialPrompt {
        prompt: agent_input.prompt,
        history: Vec::new(),
        now: SystemClock::default().now(),
    });
    let report = wants_report_pipeline(&normalized);

    if stream {
        openai_stream_response(
            &state,
            report,
            initial,
            prefix,
            degradation_prefix,
            &decision.effective_data_grant,
            id,
            model,
            created,
            include_usage,
            audit,
            audit_ctx,
            started,
        )
        .await
    } else {
        openai_buffered_response(
            &state,
            report,
            initial,
            &prefix,
            &degradation_prefix,
            &decision.effective_data_grant,
            &id,
            &model,
            created,
            audit,
            audit_ctx,
            started,
        )
        .await
    }
}

/// Shape a guardrail refusal as an OpenAI `200` (spec Data Flow: Refused → 200 + copy as the whole
/// assistant answer).
///
/// When streaming with `include_usage` (OpenAI `stream_options.include_usage`), a terminal
/// usage-only chunk is appended before `[DONE]`. A refusal spends no LLM tokens, so that usage is
/// zero (finding #4).
fn openai_refusal(
    stream: bool,
    copy: String,
    include_usage: bool,
    id: &str,
    model: &str,
    created: i64,
    refusal_code: Option<&str>,
) -> Response {
    if stream {
        let chunks = openai::build_chunks(&copy, id, model, created, include_usage);
        // A refusal costs no LLM tokens → the usage-only chunk (if requested) carries zeros.
        let usage_chunk = include_usage
            .then(|| openai::usage_chunk(openai::Usage::default(), id, model, created));
        let sse = async_stream::stream! {
            for chunk in chunks {
                yield Ok::<_, Infallible>(openai_chunk_event(&chunk));
            }
            if let Some(chunk) = usage_chunk {
                yield Ok::<_, Infallible>(openai_chunk_event(&chunk));
            }
            yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
        };
        Sse::new(sse)
            .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
            .into_response()
    } else {
        // A refusal costs no LLM tokens → usage is zero.
        let mut response =
            openai::build_response(&copy, id, model, created, openai::Usage::default());
        response.x_refusal_code = refusal_code.map(str::to_string);
        Json(response).into_response()
    }
}

/// Non-streaming path (D2): run the pipeline through a collecting sink and return one
/// `chat.completion`, then write the post-response audit (parity with `agent_stream`).
///
/// This drives the **streaming** client + drain (the same shape as [`openai_stream_response`],
/// just collected in-process instead of re-emitted as SSE) rather than the buffered `run()`,
/// because only the streaming client requests `include_usage` and thus emits the per-stage
/// `AgentEvent::Usage` this endpoint sums into the OpenAI `usage` (spec D3 — the buffered
/// `OpenAiLlm` emits none, which is why the old `run()` path reported all-zero usage).
///
/// Memory is inert on this endpoint (`session_id` is always `None`), so the memory-append side
/// effect that `agent_stream` performs is intentionally omitted; only the audit is mirrored.
#[allow(clippy::too_many_arguments)]
async fn openai_buffered_response(
    state: &AppState,
    report: bool,
    initial: AgentPayload,
    prefix: &str,
    degradation_prefix: &str,
    effective_data_grant: &[String],
    id: &str,
    model: &str,
    created: i64,
    audit: AuditWriter,
    audit_ctx: AuditCtx,
    started: Instant,
) -> Response {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(INSIGHT_STREAM_BUFFER);
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));

    let (orchestrator, pipeline_id) = match build_openai_pipeline(
        state,
        report,
        effective_data_grant,
        Some(sink.clone()),
    ) {
        Ok(pair) => pair,
        Err(e) => {
            // Finding #6: a pipeline-construction failure is a ResponseFailed too — audit it before
            // the 502, so this early return has the same audit trail as a stage failure below
            // (warn-only, parity with the success/failure paths).
            let message = format!("{e:#}");
            let duration_ms = started.elapsed().as_millis() as u64;
            if let Err(err) = audit
                .write(
                    &audit_ctx,
                    AuditEvent::ResponseFailed {
                        error_code: message.clone(),
                        duration_ms,
                    },
                )
                .await
            {
                warn!(error = %err, "chat_completions: audit ResponseFailed (pipeline build) failed");
            }
            return openai_error(StatusCode::BAD_GATEWAY, openai::ERR_UPSTREAM, message);
        }
    };

    // The task owns the orchestrator + sink; when the run finishes every sink clone drops and the
    // channel closes, so the drain loop below ends naturally.
    let run = tokio::spawn(async move {
        orchestrator
            .run_emitting(&pipeline_id, initial, &*sink)
            .await
    });

    // Drain the (lossy) event channel for per-stage token usage only (D3). The answer and any
    // failure come from the awaited run result below — never from `Finished` / `Error` events,
    // which `ChannelSink::try_send` may silently drop on a full buffer and which the
    // unknown-pipeline early return never emits at all (finding #3).
    let mut usages: Vec<UsageData> = Vec::new();
    while let Some(event) = rx.recv().await {
        if let AgentEvent::Usage {
            prompt,
            completion,
            reasoning,
            total,
        } = event
        {
            usages.push(UsageData {
                prompt,
                completion,
                reasoning,
                total,
            });
        }
    }
    // Channel closed → the run finished. Its result is authoritative for the answer / failure.
    let result = match run.await {
        Ok(inner) => resolve_outcome(inner),
        Err(join) => Err(format!("agent task failed: {join}")),
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(answer) => {
            // Prepend the answer-policy disclaimer, if any (finding #7).
            let answer = with_prefix(degradation_prefix, with_prefix(prefix, answer));
            let usage = openai::accumulate_usage(&usages);
            if let Err(e) = audit
                .write(
                    &audit_ctx,
                    AuditEvent::ResponseCompleted {
                        response_hash: hash_identifier(&answer),
                        response_chars: answer.chars().count(),
                        duration_ms,
                        status: "completed".to_string(),
                    },
                )
                .await
            {
                warn!(error = %e, "chat_completions: audit ResponseCompleted failed");
            }
            Json(openai::build_response(&answer, id, model, created, usage)).into_response()
        }
        Err(message) => {
            // A stage failed (ERR5), the pipeline produced no final result, or the task panicked
            // (parity with the stream path: surfaced as an upstream error).
            if let Err(e) = audit
                .write(
                    &audit_ctx,
                    AuditEvent::ResponseFailed {
                        error_code: message.clone(),
                        duration_ms,
                    },
                )
                .await
            {
                warn!(error = %e, "chat_completions: audit ResponseFailed failed");
            }
            openai_error(StatusCode::BAD_GATEWAY, openai::ERR_UPSTREAM, message)
        }
    }
}

/// Streaming path (D1, pseudo-streaming): run the pipeline emitting events, accumulate per-stage
/// usage, and on the terminal complete answer split it into `chat.completion.chunk`s + `[DONE]`.
///
/// When `include_usage` is set (OpenAI `stream_options.include_usage`), a terminal usage-only chunk
/// is sent after the content chunks and before `[DONE]`. The post-stream audit is written at the
/// tail of the stream (parity with `agent_stream`); memory is inert here (`session_id` is always
/// `None`), so only the audit side effect is mirrored — the memory append is omitted.
// `async` only so the pipeline-construction failure below can `await` its audit write (finding #6);
// the success path still returns the SSE handle without awaiting, so nothing blocks the stream.
#[allow(clippy::too_many_arguments)]
async fn openai_stream_response(
    state: &AppState,
    report: bool,
    initial: AgentPayload,
    prefix: String,
    degradation_prefix: String,
    effective_data_grant: &[String],
    id: String,
    model: String,
    created: i64,
    include_usage: bool,
    audit: AuditWriter,
    audit_ctx: AuditCtx,
    started: Instant,
) -> Response {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(INSIGHT_STREAM_BUFFER);
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));

    let (orchestrator, pipeline_id) = match build_openai_pipeline(
        state,
        report,
        effective_data_grant,
        Some(sink.clone()),
    ) {
        Ok(pair) => pair,
        Err(e) => {
            // Finding #6: audit the pipeline-construction failure before the 502, matching the
            // buffered path and the stage-failure audit at the stream's tail (warn-only).
            let message = format!("{e:#}");
            let duration_ms = started.elapsed().as_millis() as u64;
            if let Err(err) = audit
                .write(
                    &audit_ctx,
                    AuditEvent::ResponseFailed {
                        error_code: message.clone(),
                        duration_ms,
                    },
                )
                .await
            {
                warn!(error = %err, "chat_completions stream: audit ResponseFailed (pipeline build) failed");
            }
            return openai_error(StatusCode::BAD_GATEWAY, openai::ERR_UPSTREAM, message);
        }
    };

    // The task owns the orchestrator + sink; when the run finishes every sink clone drops and the
    // channel closes, so the drain loop below ends naturally.
    let run = tokio::spawn(async move {
        orchestrator
            .run_emitting(&pipeline_id, initial, &*sink)
            .await
    });

    let sse_stream = async_stream::stream! {
        // Drain the (lossy) event channel for per-stage token usage only (D3). The answer / failure
        // come from the awaited run result below, which is authoritative and never dropped; the
        // intermediate `ContentDelta` previews are intentionally ignored (D1, pseudo-streaming).
        // See finding #3.
        let mut usages: Vec<UsageData> = Vec::new();
        while let Some(event) = rx.recv().await {
            if let AgentEvent::Usage {
                prompt,
                completion,
                reasoning,
                total,
            } = event
            {
                usages.push(UsageData {
                    prompt,
                    completion,
                    reasoning,
                    total,
                });
            }
        }
        // Channel closed → the run finished. Its result is authoritative for the answer / failure.
        let result = match run.await {
            Ok(inner) => resolve_outcome(inner),
            Err(join) => Err(format!("agent task failed: {join}")),
        };

        let duration_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(answer) => {
                // Prepend the answer-policy disclaimer, if any (finding #7).
                let answer = with_prefix(
                    &degradation_prefix,
                    with_prefix(&prefix, answer),
                );
                let usage = openai::accumulate_usage(&usages);
                tracing::info!(
                    prompt_tokens = usage.prompt_tokens,
                    completion_tokens = usage.completion_tokens,
                    total_tokens = usage.total_tokens,
                    "chat_completions stream usage"
                );
                for chunk in openai::build_chunks(&answer, &id, &model, created, include_usage) {
                    yield Ok::<_, Infallible>(openai_chunk_event(&chunk));
                }
                // OpenAI `stream_options.include_usage`: a terminal usage-only chunk (empty
                // `choices`, populated `usage`) after the content chunks, before `[DONE]`.
                if include_usage {
                    let chunk = openai::usage_chunk(usage, &id, &model, created);
                    yield Ok::<_, Infallible>(openai_chunk_event(&chunk));
                }
                // ── post-stream audit (parity with agent_stream) ──
                if let Err(e) = audit
                    .write(
                        &audit_ctx,
                        AuditEvent::ResponseCompleted {
                            response_hash: hash_identifier(&answer),
                            response_chars: answer.chars().count(),
                            duration_ms,
                            status: "completed".to_string(),
                        },
                    )
                    .await
                {
                    warn!(error = %e, "chat_completions stream: audit ResponseCompleted failed");
                }
                yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
            }
            Err(message) => {
                // A stage failed (ERR5), the pipeline produced no final result, or the task
                // panicked. Response headers are already `200` (SSE), so surface the failure in-band
                // as an OpenAI error object, then `[DONE]`.
                if let Err(e) = audit
                    .write(
                        &audit_ctx,
                        AuditEvent::ResponseFailed {
                            error_code: message.clone(),
                            duration_ms,
                        },
                    )
                    .await
                {
                    warn!(error = %e, "chat_completions stream: audit ResponseFailed failed");
                }
                let body = openai::OpenAiErrorBody::new(openai::ERR_UPSTREAM, message);
                yield Ok::<_, Infallible>(
                    Event::default()
                        .json_data(&body)
                        .expect("OpenAiErrorBody is always valid JSON"),
                );
                yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
            }
        }
    };

    Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
        .into_response()
}

// ──── sub-agent pipeline shared helpers ───

/// Bounded buffer for the sub-agent pipeline event channel.
///
/// Large enough that a normal turn's events (stage transitions plus the analyst's streamed
/// report) are never dropped by [`ChannelSink`]'s `try_send`; a lossless channel is a later
/// refinement (plan §8.2).
const INSIGHT_STREAM_BUFFER: usize = 8192;

/// Map one sub-agent [`AgentEvent`] onto the external SSE frames it surfaces (0 or more).
///
/// Each stage's start and completion surface as `stage` frames — the completion carrying a
/// `success` / `failure` phase, for a green/red indicator — and every LLM stage's tokens stream
/// live as `token`s (delimited by those stage frames). While the model composes a tool call, its
/// argument fragments stream as `tool_args` frames and the assembled call surfaces as a `tool_call`
/// frame, so a long tool-calling turn (e.g. the fetcher querying the datacenter, or the composer
/// building the report payload) keeps signalling that the task is still running. On the terminal
/// `Finished` the **complete** answer (report + charts, or the rendered HTML) is re-sent after a
/// `clear`, so a consumer always ends with the correct full answer regardless of the intermediate
/// previews. Each LLM turn's token usage surfaces as a `usage` frame — including the hidden
/// reasoning tokens, so a truncation is explained rather than mysterious. The remaining tool /
/// reasoning events (`ToolStarted`, `ToolProduced`, `ReasoningDelta`, `StageProduced`) stay internal
/// for now.
fn insight_frames(event: AgentEvent) -> Vec<StreamFrame> {
    match event {
        AgentEvent::StageStarted { agent, .. } => vec![StreamFrame::Stage {
            data: StageData {
                agent: agent.0,
                phase: StagePhase::Started,
            },
        }],
        AgentEvent::StageFinished { agent, outcome } => vec![StreamFrame::Stage {
            data: StageData {
                agent: agent.0,
                phase: stage_phase(outcome),
            },
        }],
        AgentEvent::ContentDelta { text } => vec![StreamFrame::Token { data: text }],
        // Live tool-call progress: argument fragments stream as they arrive; the assembled call
        // (with its tool name) follows once the model finishes composing it.
        AgentEvent::ToolArgsDelta { id, fragment } => vec![StreamFrame::ToolArgs {
            data: ToolArgsData { id, fragment },
        }],
        AgentEvent::ToolCallProposed { id, name } => vec![StreamFrame::ToolCall {
            data: ToolCallData { id, name },
        }],
        // Per-turn token accounting — surfaces the hidden reasoning-token budget behind a silent
        // burn, so a truncation is explained rather than mysterious.
        AgentEvent::Usage {
            prompt,
            completion,
            reasoning,
            total,
        } => vec![StreamFrame::Usage {
            data: UsageData {
                prompt,
                completion,
                reasoning,
                total,
            },
        }],
        AgentEvent::Finished { assistant } => vec![
            StreamFrame::Clear,
            StreamFrame::Token { data: assistant },
            StreamFrame::Done,
        ],
        AgentEvent::Error { message } => vec![StreamFrame::Error {
            data: message,
            code: crate::server::codes::UPSTREAM_ERROR.to_string(),
        }],
        // Internal framing (stage produced, tool execution, reasoning deltas) is not surfaced to
        // the browser yet.
        _ => vec![],
    }
}

/// Map a stage's [`StageOutcome`] onto its wire [`StagePhase`].
fn stage_phase(outcome: StageOutcome) -> StagePhase {
    match outcome {
        StageOutcome::Success => StagePhase::Success,
        StageOutcome::Failure => StagePhase::Failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;

    #[test]
    fn insight_frames_stream_stages_tokens_and_a_clean_terminal() {
        use crate::agent::config::SubAgentId;
        use crate::agent::payload::PayloadKind;

        // A stage starting surfaces which sub-agent is running (a spinning dot).
        assert_eq!(
            insight_frames(AgentEvent::StageStarted {
                agent: SubAgentId("analyst".into()),
                input: PayloadKind::Intermediate,
            }),
            vec![StreamFrame::Stage {
                data: StageData {
                    agent: "analyst".into(),
                    phase: StagePhase::Started,
                }
            }]
        );
        // A stage finishing carries its outcome — success (green) or failure (red).
        assert_eq!(
            insight_frames(AgentEvent::StageFinished {
                agent: SubAgentId("fetcher".into()),
                outcome: StageOutcome::Success,
            }),
            vec![StreamFrame::Stage {
                data: StageData {
                    agent: "fetcher".into(),
                    phase: StagePhase::Success,
                }
            }]
        );
        assert_eq!(
            insight_frames(AgentEvent::StageFinished {
                agent: SubAgentId("charter".into()),
                outcome: StageOutcome::Failure,
            }),
            vec![StreamFrame::Stage {
                data: StageData {
                    agent: "charter".into(),
                    phase: StagePhase::Failure,
                }
            }]
        );
        // Every LLM stage's output streams live as tokens.
        assert_eq!(
            insight_frames(AgentEvent::ContentDelta { text: "hi".into() }),
            vec![StreamFrame::Token { data: "hi".into() }]
        );
        // A tool call's argument fragments stream live (progress during a long tool-calling turn)…
        assert_eq!(
            insight_frames(AgentEvent::ToolArgsDelta {
                id: "call_1".into(),
                fragment: "{\"seller".into(),
            }),
            vec![StreamFrame::ToolArgs {
                data: ToolArgsData {
                    id: "call_1".into(),
                    fragment: "{\"seller".into(),
                }
            }]
        );
        // …and the assembled call surfaces its tool name so the client can label it.
        assert_eq!(
            insight_frames(AgentEvent::ToolCallProposed {
                id: "call_1".into(),
                name: "bill_revenue".into(),
            }),
            vec![StreamFrame::ToolCall {
                data: ToolCallData {
                    id: "call_1".into(),
                    name: "bill_revenue".into(),
                }
            }]
        );
        // A turn's token usage surfaces the hidden reasoning budget.
        assert_eq!(
            insight_frames(AgentEvent::Usage {
                prompt: 1200,
                completion: 8000,
                reasoning: Some(7600),
                total: 9200,
            }),
            vec![StreamFrame::Usage {
                data: UsageData {
                    prompt: 1200,
                    completion: 8000,
                    reasoning: Some(7600),
                    total: 9200,
                }
            }]
        );
        // The terminal frame re-sends the COMPLETE answer after a clear, so the finalizer's
        // appended charts (absent from the streamed preview) always reach the client.
        assert_eq!(
            insight_frames(AgentEvent::Finished {
                assistant: "full answer".into()
            }),
            vec![
                StreamFrame::Clear,
                StreamFrame::Token {
                    data: "full answer".into()
                },
                StreamFrame::Done,
            ]
        );
        assert_eq!(
            insight_frames(AgentEvent::Error {
                message: "boom".into()
            }),
            vec![StreamFrame::Error {
                data: "boom".into(),
                code: crate::server::codes::UPSTREAM_ERROR.into(),
            }]
        );
        // Internal framing (stage produced) surfaces nothing on the wire for now.
        assert!(insight_frames(AgentEvent::StageProduced {
            agent: SubAgentId("fetcher".into()),
            keys: vec![],
        })
        .is_empty());
    }

    // ── /agent/stream intent routing ──

    fn normalized_with(intent: &str, candidates: &[&str]) -> NormalizedInput {
        NormalizedInput {
            prompt: String::new(),
            intent: intent.to_string(),
            confidence: 1.0,
            candidate_intents: candidates.iter().map(|s| s.to_string()).collect(),
            intent_source: None,
            slots: Default::default(),
            warnings: Vec::new(),
        }
    }

    #[test]
    fn wants_report_pipeline_on_top_intent_or_candidate_only() {
        // Report as the resolved top intent.
        assert!(wants_report_pipeline(&normalized_with(
            "report",
            &["report"]
        )));
        // Report merely mentioned (a candidate) while a topic won the top slot —
        // e.g. `營收報告`: revenue is top, report co-occurs.
        assert!(wants_report_pipeline(&normalized_with(
            "revenue",
            &["revenue", "report"]
        )));
        // No report anywhere → insight pipeline.
        assert!(!wants_report_pipeline(&normalized_with(
            "revenue",
            &["revenue"]
        )));
        assert!(!wants_report_pipeline(&normalized_with("unknown", &[])));
    }

    #[test]
    fn status_to_app_error_maps_4xx_to_bad_request_else_unavailable() {
        assert!(matches!(
            status_to_app_error(400, "input_required".into()),
            AppError::BadRequest(_)
        ));
        assert!(matches!(
            status_to_app_error(500, "boom".into()),
            AppError::ServiceUnavailable(_)
        ));
    }

    /// End-to-end: the real runtime intent pipeline classifies report vocabulary,
    /// and `wants_report_pipeline` routes it — proving the `intents.toml` wiring.
    #[test]
    fn report_vocabulary_routes_to_report_pipeline_via_runtime_config() {
        use crate::config::AppConfig;
        use crate::runtime::config::RuntimeConfig;
        use crate::runtime::input::pipeline::InputPipeline;
        use crate::runtime::registry::BuiltinRegistry;

        let refs = AppConfig::load("config/config.toml")
            .expect("app config should load")
            .runtime
            .expect("runtime refs should exist");
        let cfg =
            RuntimeConfig::load(&refs, &BuiltinRegistry::default()).expect("runtime config loads");
        let pipeline = InputPipeline::default();
        let classify = |prompt: &str| {
            pipeline
                .run_with_config(&cfg, prompt, None)
                .expect("input pipeline runs")
        };

        // A bare report ask → report pipeline.
        assert!(wants_report_pipeline(&classify("給我一份完整的報告")));
        // A topic + report ask (`營收報告`) → still the report pipeline (report is a
        // candidate), while the topic rides along in the prompt.
        assert!(wants_report_pipeline(&classify("我想要營收報告")));
        // A plain analytics ask (no report vocabulary) → insight pipeline.
        assert!(!wants_report_pipeline(&classify("分析最近三個月的營收")));
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-009
    async fn authorized_report_crosses_runtime_pipeline_with_terminal_degradation_notice() {
        // T11/T17: exercise the real handler → runtime → grant-aware report pipeline → MCP
        // boundary with only local scripted transports. Finance can read two report tools, so the
        // report is allowed but must declare the omitted charging/member topics in its final answer.
        let llm = crate::test_support::ScriptedChatCompletions::start();
        let provider = crate::test_support::ScriptedPermissionsProvider::documented(
            123,
            &["hdrenewables/elecsvc/starcharger/finance"],
        );
        let (state, mcp) = crate::test_support::runtime_app_state_with_fixtures(
            provider,
            llm.base_url.clone(),
            &[
                "bill_revenue",
                "bill_charge",
                "member_analysis",
                "business_metrics",
                "station_revenue_ranking",
                "bill_member_analysis",
            ],
        )
        .await;
        let app = crate::server::route::build_router(state);
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header(
                        "authorization",
                        format!("Bearer {}", crate::test_support::TEST_TOKEN),
                    )
                    .header("x-falcon-authorization", "Bearer delegated-local-token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "model": "test/model",
                            "messages": [{"role": "user", "content": "給我一份完整的報告"}],
                            "stream": false
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .expect("router request should complete");

        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_str(&body_string(response).await)
            .expect("OpenAI response should be JSON");
        let answer = body["choices"][0]["message"]["content"]
            .as_str()
            .expect("response should contain an assistant answer");
        assert!(answer.contains("本報告因權限不足，省略以下主題"));
        assert!(answer.contains("falcon-report"));
        assert_eq!(
            mcp.tool_calls(),
            1,
            "only the granted data tool may be called"
        );
        assert!(
            llm.requests() >= 5,
            "fetcher/analyst/composer calls should be scripted"
        );
    }

    // ── /v1/chat/completions helpers (findings #3/#4/#6/#7) ──

    #[test]
    fn json_rejection_status_keeps_413_and_415_else_400() {
        // Body over the limit → 413; wrong/absent content-type → 415; everything else → 400.
        assert_eq!(
            json_rejection_status(StatusCode::PAYLOAD_TOO_LARGE),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            json_rejection_status(StatusCode::UNSUPPORTED_MEDIA_TYPE),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        // JSON syntax error (400) and wrong-shape (422) both collapse to 400.
        assert_eq!(
            json_rejection_status(StatusCode::BAD_REQUEST),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            json_rejection_status(StatusCode::UNPROCESSABLE_ENTITY),
            StatusCode::BAD_REQUEST
        );
    }

    #[tokio::test]
    async fn real_json_rejections_map_to_413_415_and_400() {
        // Drive the actual axum `Json` extractor rejection (no AppState needed) to confirm
        // `rejection.status()` yields the codes `json_rejection_status` keys off (finding #6).
        use axum::extract::rejection::JsonRejection;
        use axum::extract::DefaultBodyLimit;
        use axum::routing::post;
        use tower::ServiceExt;

        // A probe that maps a JsonRejection exactly as `chat_completions` does.
        async fn probe(payload: Result<Json<serde_json::Value>, JsonRejection>) -> Response {
            match payload {
                Ok(_) => StatusCode::OK.into_response(),
                Err(rej) => json_rejection_status(rej.status()).into_response(),
            }
        }
        let app = axum::Router::new()
            .route("/p", post(probe))
            .layer(DefaultBodyLimit::max(16));

        // Body over the 16-byte limit → 413.
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/p")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        "{\"a\":\"01234567890123456789\"}".to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);

        // Wrong content-type → 415.
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/p")
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from("hi".to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        // Malformed JSON within the limit → 400.
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/p")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{".to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn with_prefix_prepends_a_nonempty_disclaimer_only() {
        assert_eq!(with_prefix("", "answer".into()), "answer");
        assert_eq!(
            with_prefix("disclaimer", "answer".into()),
            "disclaimer\n\nanswer"
        );
    }

    #[test]
    fn resolve_outcome_reads_answer_and_failures_from_the_run_result() {
        use crate::agent::payload::FinalResult;
        use std::collections::HashMap;

        let now = chrono::DateTime::parse_from_rfc3339("2026-07-23T00:00:00+00:00").unwrap();
        // A Final payload → its assistant text is the authoritative answer.
        let final_ok = Ok(AgentPayload::Final(FinalResult {
            user: "u".into(),
            assistant: "THE ANSWER".into(),
            now,
            artifacts: HashMap::new(),
        }));
        assert_eq!(resolve_outcome(final_ok), Ok("THE ANSWER".into()));

        // A capability error (incl. the unknown-pipeline early return, which emits NO Error event)
        // surfaces its Display as the failure — the drain-only path would have missed it entirely.
        let cap_err = Err(AgentError::Capability("unknown pipeline: p".into()));
        assert_eq!(
            resolve_outcome(cap_err),
            Err("capability error: unknown pipeline: p".into())
        );

        // A non-Final terminal payload is a wiring fault, not a silent success.
        let now2 = now;
        let not_final = Ok(AgentPayload::Initial(InitialPrompt {
            prompt: "x".into(),
            history: vec![],
            now: now2,
        }));
        assert!(resolve_outcome(not_final).is_err());
    }

    /// Collect the payloads of every `data: <payload>` line of an SSE body, in order.
    fn sse_data_lines(body: &str) -> Vec<String> {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(str::to_string)
            .collect()
    }

    async fn body_string(resp: Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("read body");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    }

    #[tokio::test]
    async fn refusal_stream_appends_a_zero_usage_chunk_when_include_usage() {
        // AC-5 / finding #4: a streamed refusal honours `stream_options.include_usage` by emitting a
        // terminal usage-only chunk (zero cost — a refusal spends no LLM tokens) before `[DONE]`.
        let resp = openai_refusal(true, "refused copy".into(), true, "id", "m", 0, None);
        let lines = sse_data_lines(&body_string(resp).await);

        assert_eq!(
            lines.last().unwrap(),
            "[DONE]",
            "stream must end with [DONE]"
        );
        // The chunk immediately before [DONE] is the usage-only chunk: empty choices, zero usage.
        let usage_chunk: serde_json::Value =
            serde_json::from_str(&lines[lines.len() - 2]).expect("usage chunk is JSON");
        assert!(usage_chunk["choices"].as_array().unwrap().is_empty());
        assert_eq!(usage_chunk["usage"]["prompt_tokens"], 0);
        assert_eq!(usage_chunk["usage"]["completion_tokens"], 0);
        assert_eq!(usage_chunk["usage"]["total_tokens"], 0);
        // The content chunks (everything before the usage chunk) reconstruct the refusal copy.
        let content: String = lines[..lines.len() - 2]
            .iter()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|c| {
                c["choices"][0]["delta"]["content"]
                    .as_str()
                    .map(String::from)
            })
            .collect();
        assert_eq!(content, "refused copy");
    }

    #[tokio::test]
    async fn refusal_stream_omits_usage_chunk_without_include_usage() {
        // Wire unchanged when the client did not opt in: no chunk carries a `usage` field.
        let resp = openai_refusal(true, "refused".into(), false, "id", "m", 0, None);
        let lines = sse_data_lines(&body_string(resp).await);

        assert_eq!(lines.last().unwrap(), "[DONE]");
        for line in &lines {
            if *line == "[DONE]" {
                continue;
            }
            let chunk: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(
                chunk.get("usage").is_none(),
                "no chunk may carry usage when include_usage is off, got {line}"
            );
        }
    }

    #[tokio::test]
    async fn refusal_non_stream_returns_a_chat_completion_with_the_copy() {
        // Non-streaming refusal: a 200 `chat.completion` whose single choice content is the copy.
        let resp = openai_refusal(false, "refused copy".into(), false, "id", "m", 0, None);
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value =
            serde_json::from_str(&body_string(resp).await).expect("json body");
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["choices"][0]["message"]["content"], "refused copy");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["total_tokens"], 0);
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-006
    async fn authorization_refusal_exposes_its_machine_code_on_openai_completion() {
        let response = openai_refusal(
            false,
            "權限不足".into(),
            false,
            "id",
            "m",
            0,
            Some(crate::server::codes::AUTHZ_INSUFFICIENT),
        );
        let body: serde_json::Value =
            serde_json::from_str(&body_string(response).await).expect("response is JSON");
        assert_eq!(
            body["x_refusal_code"],
            crate::server::codes::AUTHZ_INSUFFICIENT
        );
        assert_eq!(body["choices"][0]["message"]["content"], "權限不足");
    }
}
