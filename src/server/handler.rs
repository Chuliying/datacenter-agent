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

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::{instrument, warn, Instrument};

use rand::seq::SliceRandom;

use futures::StreamExt;

use super::dto::{
    AgentRequest, AgentResponse, GreetingResponse, IntentResolvedData, ReadyBody, ReadyChecks,
    StageData, StagePhase, StreamFrame, ToolArgsData, ToolCallData, UsageData,
};
use super::error::AppError;
use super::openai;
use super::AppState;
use crate::agent::clock::{Clock, SystemClock};
use crate::agent::config::PipelineId;
use crate::agent::engine::Orchestrator;
use crate::agent::events::{AgentEvent, ChannelSink, EventSink, StageOutcome};
use crate::agent::payload::{AgentError, AgentPayload, Exchange, InitialPrompt};
use crate::agent::pipeline::{agent_pipeline_id, report_pipeline_id};
use crate::agent::wiring::{build_insight_pipeline, build_report_pipeline};
use crate::runtime::audit::{hash_identifier, AuditCtx, AuditEvent, AuditWriter};
use crate::runtime::schema::{AgentTurnFrame, AgentTurnInput, NormalizedInput};
use crate::runtime::turn::{
    append_memory_turn_if_enabled, plan_stream_turn, AgentPort, AgentTurnDeps, StreamPlan,
    TurnEvent,
};

/// Upper bound on the user prompt, in UTF-8 characters.
pub const USER_PROMPT_LENGTH_CAP: usize = 2_000;

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

// ──── /insight ───

/// Non-streaming analytics handler: runs the four-stage `/insight` pipeline (fetcher → analyst →
/// charter → finalizer) and returns the finalizer's complete answer — the analyst's report with
/// any charts embedded as `falcon-chart` fenced blocks.
///
/// This drives the sub-agent pipeline **directly**: it does not pass through the runtime turn
/// (guardrails / intent / memory / audit). Routing the pipeline behind the runtime `AgentPort` is
/// the plan's §9 step, deferred until the pipeline is proven by hand.
pub async fn insight(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<AgentResponse>, AppError> {
    let Json(req) = req?;

    let span = tracing::info_span!(
        "insight",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
        session_id = req.session_id.as_deref().unwrap_or(""),
        option_id = req.option_id.as_deref().unwrap_or(""),
    );
    async move {
        validate_prompt(&req.prompt)?;
        let user_prompt = req.prompt.clone();

        // Assemble the pipeline buffered (no sink) and thread the payload through it.
        let resolved = state.llm.resolved();
        let orchestrator = build_insight_pipeline(
            state.mcp.clone(),
            &state.tools,
            state.instructions.as_deref(),
            &state.prompts.fetcher_system,
            &state.prompts.analyst_system,
            &state.prompts.charter_system,
            &state.insight_grants.fetcher,
            &state.insight_grants.charter,
            &resolved,
            None,
        )
        .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;

        let outcome = orchestrator
            .run(&agent_pipeline_id(), insight_initial(req))
            .await
            .map_err(insight_error_to_app_error)?;

        Ok(Json(AgentResponse {
            user_prompt,
            model_response: final_answer(outcome)?,
            intent: "unknown".into(),
        }))
    }
    .instrument(span)
    .await
}

// ──── /report ───

/// HTML report handler: runs the four-stage `/report` pipeline (fetcher → analyst → composer →
/// renderer) and returns the renderer's self-contained HTML report, wrapped in a `falcon-report`
/// fenced block.
///
/// The economy over the old monolith: the `composer` emits only the small structured
/// [`ReportData`](crate::agent::report::ReportData) via its `emit_report` sink, and the pure-logic
/// renderer injects it into the boot-loaded template — **no LLM ever writes HTML**, so the report
/// is faster, cheaper, and design-stable per turn.
///
/// Drives the pipeline directly (no runtime turn), like [`insight`].
pub async fn report(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<AgentResponse>, AppError> {
    let Json(req) = req?;

    let span = tracing::info_span!(
        "report",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
    );
    async move {
        validate_prompt(&req.prompt)?;
        let user_prompt = req.prompt.clone();

        // Assemble the pipeline buffered (no sink) and thread the payload through it.
        let resolved = state.llm.resolved();
        let orchestrator = build_report_pipeline(
            state.mcp.clone(),
            &state.tools,
            state.instructions.as_deref(),
            &state.prompts.fetcher_system,
            &state.prompts.report_analyst_system,
            &state.prompts.report_composer_system,
            &state.insight_grants.fetcher,
            &resolved,
            state.report_template.clone(),
            None,
        )
        .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;

        let outcome = orchestrator
            .run(&report_pipeline_id(), insight_initial(req))
            .await
            .map_err(insight_error_to_app_error)?;

        Ok(Json(AgentResponse {
            user_prompt,
            model_response: final_answer(outcome)?,
            intent: "unknown".into(),
        }))
    }
    .instrument(span)
    .await
}

// ──── /report/stream ───

/// Server-Sent Events variant of [`report`].
///
/// Same wire contract as [`insight_stream`] — `stage` frames (progress dots) plus a terminal
/// `clear` + full-answer `token` + `done`. The `composer` emits a tool call rather than streamable
/// prose, so the live signal is the per-stage progress; the finished HTML arrives on the terminal
/// frame.
pub async fn report_stream(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let Json(req) = req?;

    let span = tracing::info_span!(
        "report-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
    );
    let _enter = span.enter();

    validate_prompt(&req.prompt)?;

    // One shared per-turn sink drives the stream (plan §8.5, mechanism A): the streaming LLM stages
    // emit content deltas onto it, and the orchestrator emits stage transitions.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(INSIGHT_STREAM_BUFFER);
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));

    let resolved = state.llm.resolved();
    let orchestrator = build_report_pipeline(
        state.mcp.clone(),
        &state.tools,
        state.instructions.as_deref(),
        &state.prompts.fetcher_system,
        &state.prompts.report_analyst_system,
        &state.prompts.report_composer_system,
        &state.insight_grants.fetcher,
        &resolved,
        state.report_template.clone(),
        Some(sink.clone()),
    )
    .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;

    let initial = insight_initial(req);
    // The task owns the orchestrator + sink, so when the run finishes every sink clone drops and
    // the channel closes — the drain loop below ends naturally on close.
    let run = tokio::spawn(async move {
        orchestrator
            .run_emitting(&report_pipeline_id(), initial, &*sink)
            .await
    });

    let sse_stream = async_stream::stream! {
        while let Some(event) = rx.recv().await {
            for frame in insight_frames(event) {
                yield Ok::<_, Infallible>(sse_event(frame));
            }
        }
        // Channel closed → the run finished. A stage failure already surfaced as an `error` frame
        // during draining; only a task panic needs a fallback terminal frame here.
        if let Err(join) = run.await {
            yield Ok::<_, Infallible>(sse_event(StreamFrame::Error {
                data: format!("report task failed: {join}"),
            }));
        }
    };

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
        .into_response())
}

// ──── /insight/stream ───

/// Server-Sent Events variant of [`insight`].
///
/// Runs the same four-stage pipeline, streaming its progress live onto one shared per-turn sink
/// (plan §8.5, mechanism A). Each frame is a single SSE `data:` line carrying a JSON envelope:
/// - `{"event":"stage","data":{"agent":"<id>","phase":"started"}}`: the sub-agent started; on
///   completion a matching `{"phase":"success"}` / `{"phase":"failure"}` follows (a green/red dot).
/// - `{"event":"token","data":"<text fragment>"}`: a fragment of the current stage's output.
/// - `{"event":"clear"}` then `{"event":"token","data":"<full answer>"}`: on completion, the
///   streamed previews are cleared and the **complete** finalizer answer (report + charts) is
///   re-sent, so a consumer always ends with the correct full answer.
/// - `{"event":"done"}`: finished cleanly; close the connection.
/// - `{"event":"error","data":"<message>"}`: terminal error; close the connection.
///
/// Like [`insight`], this drives the pipeline directly (no runtime turn).
pub async fn insight_stream(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let Json(req) = req?;

    let span = tracing::info_span!(
        "insight-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
        session_id = req.session_id.as_deref().unwrap_or(""),
        option_id = req.option_id.as_deref().unwrap_or(""),
    );
    let _enter = span.enter();

    validate_prompt(&req.prompt)?;

    // One shared per-turn sink drives the stream: the streaming analyst LLM emits content deltas
    // onto it, and the orchestrator emits stage transitions (both from outside `SubAgent::run`).
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(INSIGHT_STREAM_BUFFER);
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));

    let resolved = state.llm.resolved();
    let orchestrator = build_insight_pipeline(
        state.mcp.clone(),
        &state.tools,
        state.instructions.as_deref(),
        &state.prompts.fetcher_system,
        &state.prompts.analyst_system,
        &state.prompts.charter_system,
        &state.insight_grants.fetcher,
        &state.insight_grants.charter,
        &resolved,
        Some(sink.clone()),
    )
    .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;

    let initial = insight_initial(req);
    // The task owns the orchestrator + sink, so when the run finishes every sink clone drops and
    // the channel closes — the drain loop below ends naturally on close.
    let run = tokio::spawn(async move {
        orchestrator
            .run_emitting(&agent_pipeline_id(), initial, &*sink)
            .await
    });

    let sse_stream = async_stream::stream! {
        while let Some(event) = rx.recv().await {
            for frame in insight_frames(event) {
                yield Ok::<_, Infallible>(sse_event(frame));
            }
        }
        // Channel closed → the run finished. A stage failure already surfaced as an `error` frame
        // during draining (the orchestrator emits it before returning `Err`); only a task panic
        // needs a fallback terminal frame here.
        if let Err(join) = run.await {
            yield Ok::<_, Infallible>(sse_event(StreamFrame::Error {
                data: format!("insight task failed: {join}"),
            }));
        }
    };

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
        .into_response())
}

// ──── /agent/stream ───

/// Intent id that routes a turn to the `/report` pipeline (defined in
/// `config/runtime/intents.toml`).
const REPORT_INTENT: &str = "report";

/// Route to the `/report` pipeline when the user asked for a report — the `report`
/// intent was *mentioned* (the resolved top intent, or present among the
/// candidates). Otherwise the `/insight` pipeline.
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
/// pipeline the resolved intent selects: the `/report` pipeline when a report was
/// asked for (see [`wants_report_pipeline`]), else `/insight`.
///
/// Unlike [`insight_stream`] / [`report_stream`] — which drive one fixed pipeline
/// directly, bypassing the runtime — this is the routed production path. It reuses
/// the runtime's [`plan_stream_turn`] prelude verbatim (no duplicated
/// guardrail/intent logic), then streams the chosen pipeline's rich stage frames
/// through the same [`insight_frames`] mapping — so the stage-aware SSE contract is
/// identical — and replicates the runtime turn's two post-stream side effects
/// (`ResponseCompleted` / `ResponseFailed` audit + session-memory append).
///
/// Requires the runtime to be enabled (`RUNTIME_ENABLED`, default on); rolled back,
/// this returns `503` and callers should use the direct `/insight/stream` or
/// `/report/stream` pipelines.
pub async fn agent_stream(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let Json(req) = req?;

    let runtime = state
        .runtime
        .clone()
        .filter(|rt| rt.enabled)
        .ok_or_else(|| {
            AppError::ServiceUnavailable(
                "runtime disabled (RUNTIME_ENABLED=false); use /insight/stream or /report/stream"
                    .into(),
            )
        })?;

    let span = tracing::info_span!(
        "agent-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
        session_id = req.session_id.as_deref().unwrap_or(""),
        option_id = req.option_id.as_deref().unwrap_or(""),
    );

    async move {
        // ── runtime prelude: audit, guardrails, intent, answer policy, memory ──
        // Reuses the runtime turn's synchronous prelude verbatim. The dummy port +
        // no-op emit are never exercised by `plan_stream_turn`; this handler owns
        // the streaming itself.
        let request_id = uuid::Uuid::new_v4();
        let audit_ctx = AuditCtx {
            request_id: request_id.to_string(),
            session_id: req.session_id.clone(),
            route: "/agent/stream".into(),
            actor: None,
        };
        let input = AgentTurnInput {
            request_id,
            prompt: req.prompt.clone(),
            raw_input: req.prompt.clone(),
            history: req.history.clone(),
            session_id: req.session_id.clone(),
            option_id: req.option_id.clone(),
        };
        let audit = AuditWriter::new(runtime.audit_sink.clone(), runtime.audit_failure_policy);

        let plan = {
            let unused = UnusedAgentPort;
            let emit_noop = |_event: TurnEvent| {};
            let deps = AgentTurnDeps {
                runtime_config: &runtime.config,
                input_pipeline: &runtime.input_pipeline,
                answer_policy: runtime.answer_policy.as_ref(),
                llm_normalizer: runtime.llm_normalizer.as_deref(),
                sessions: runtime.sessions.as_deref(),
                agent: &unused,
                audit: &audit,
                emit: &emit_noop,
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
            } => (started, prefix, agent_input, *normalized),
        };

        // ── build the intent-selected pipeline (streaming) ──
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(INSIGHT_STREAM_BUFFER);
        let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));
        let resolved = state.llm.resolved();

        let (orchestrator, pipeline_id) = if wants_report_pipeline(&normalized) {
            let orch = build_report_pipeline(
                state.mcp.clone(),
                &state.tools,
                state.instructions.as_deref(),
                &state.prompts.fetcher_system,
                &state.prompts.report_analyst_system,
                &state.prompts.report_composer_system,
                &state.insight_grants.fetcher,
                &resolved,
                state.report_template.clone(),
                Some(sink.clone()),
            )
            .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
            (orch, report_pipeline_id())
        } else {
            let orch = build_insight_pipeline(
                state.mcp.clone(),
                &state.tools,
                state.instructions.as_deref(),
                &state.prompts.fetcher_system,
                &state.prompts.analyst_system,
                &state.prompts.charter_system,
                &state.insight_grants.fetcher,
                &state.insight_grants.charter,
                &resolved,
                Some(sink.clone()),
            )
            .map_err(|e| AppError::BadGateway(format!("{e:#}")))?;
            (orch, agent_pipeline_id())
        };

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
                        response = assistant.clone();
                        completed = true;
                    }
                    AgentEvent::Error { message } => failure = Some(message.clone()),
                    _ => {}
                }
                for frame in insight_frames(event) {
                    yield Ok::<_, Infallible>(sse_event(frame));
                }
            }

            // Channel closed → the run finished. A stage failure already surfaced as
            // an `error` frame during draining; only a task panic needs a fallback.
            if let Err(join) = run.await {
                yield Ok::<_, Infallible>(sse_event(StreamFrame::Error {
                    data: format!("agent task failed: {join}"),
                }));
            }

            // ── post-stream side effects (parity with the runtime turn) ──
            let duration_ms = started.elapsed().as_millis() as u64;
            if let Some(error_code) = failure {
                if let Err(e) = audit
                    .write(&audit_ctx, AuditEvent::ResponseFailed { error_code, duration_ms })
                    .await
                {
                    warn!(error = %e, "agent-stream: audit ResponseFailed failed");
                }
            } else if completed {
                if let Err(e) = append_memory_turn_if_enabled(
                    &memory_input,
                    sessions.as_deref(),
                    &normalized,
                    &response,
                )
                .await
                {
                    warn!(error = %e, "agent-stream: memory append failed");
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
                    warn!(error = %e, "agent-stream: audit ResponseCompleted failed");
                }
            }
            // else: aborted (no Finished/Error) — mirror `stream_agent_response` (no extra audit).
        };

        Ok(Sse::new(sse_stream)
            .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
            .into_response())
    }
    .instrument(span)
    .await
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
    (status, Json(openai::OpenAiErrorBody::new(error_type, message))).into_response()
}

/// Serialize one `chat.completion.chunk` into an SSE `data:` line — a pure `data:` stream with no
/// `event:` name, as OpenAI expects (spec D1).
fn openai_chunk_event(chunk: &openai::ChatCompletionChunk) -> Event {
    Event::default()
        .json_data(chunk)
        .expect("chat.completion.chunk is always valid JSON")
}

/// Build the intent-selected sub-agent pipeline for the OpenAI endpoint (spec Data Flow): the
/// `/report` pipeline when a report was asked for (see [`wants_report_pipeline`]), else `/insight`.
/// `sink = Some(_)` selects the streaming shape; `None` is buffered (spec D1/D2).
fn build_openai_pipeline(
    state: &AppState,
    report: bool,
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
            &state.insight_grants.fetcher,
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
            &state.insight_grants.fetcher,
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
/// Auth is inherited from the shared `require_bearer` layer (D6, `418` on a bad token); the runtime
/// is required (D7, `503` when `RUNTIME_ENABLED=false`). Errors use the OpenAI envelope (spec
/// Errors). `session_id` / `option_id` have no OpenAI equivalent, so server-side memory is inert.
#[instrument(skip_all, fields(route = "/v1/chat/completions"))]
pub async fn chat_completions(
    State(state): State<AppState>,
    req: Result<Json<openai::ChatCompletionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // ── parse body (ERR2: malformed JSON / missing fields) ──
    let Json(req) = match req {
        Ok(json) => json,
        Err(rejection) => {
            return openai_error(
                StatusCode::BAD_REQUEST,
                openai::ERR_INVALID_REQUEST,
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
        actor: None,
    };
    let input = AgentTurnInput {
        request_id,
        prompt: agent_req.prompt.clone(),
        raw_input: agent_req.prompt.clone(),
        history: agent_req.history.clone(),
        session_id: agent_req.session_id.clone(),
        option_id: agent_req.option_id.clone(),
    };
    let audit = AuditWriter::new(runtime.audit_sink.clone(), runtime.audit_failure_policy);

    let plan = {
        let unused = UnusedAgentPort;
        let emit_noop = |_event: TurnEvent| {};
        let deps = AgentTurnDeps {
            runtime_config: &runtime.config,
            input_pipeline: &runtime.input_pipeline,
            answer_policy: runtime.answer_policy.as_ref(),
            llm_normalizer: runtime.llm_normalizer.as_deref(),
            sessions: runtime.sessions.as_deref(),
            agent: &unused,
            audit: &audit,
            emit: &emit_noop,
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
    let (started, agent_input, normalized) = match plan {
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
            return openai_refusal(stream, copy, &id, &model, created);
        }
        // The disclaimer `prefix` is intentionally dropped: on /agent/stream the pipeline's terminal
        // `clear` supersedes it, so the delivered answer never carries it — matched here. `started`
        // is the turn's start instant (from the prelude), carried forward for the post-response
        // audit's `duration_ms` (parity with `agent_stream`).
        StreamPlan::Proceed {
            started,
            agent_input,
            normalized,
            ..
        } => (started, agent_input, *normalized),
    };

    // ── build + run the intent-selected pipeline ──
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
    let report = wants_report_pipeline(&normalized);

    if stream {
        openai_stream_response(
            &state,
            report,
            initial,
            id,
            model,
            created,
            include_usage,
            audit,
            audit_ctx,
            started,
        )
    } else {
        openai_buffered_response(
            &state, report, initial, &id, &model, created, audit, audit_ctx, started,
        )
        .await
    }
}

/// Shape a guardrail refusal as an OpenAI `200` (spec Data Flow: Refused → 200 + copy as the whole
/// assistant answer).
fn openai_refusal(stream: bool, copy: String, id: &str, model: &str, created: i64) -> Response {
    if stream {
        let chunks = openai::build_chunks(&copy, id, model, created);
        let sse = async_stream::stream! {
            for chunk in chunks {
                yield Ok::<_, Infallible>(openai_chunk_event(&chunk));
            }
            yield Ok::<_, Infallible>(Event::default().data("[DONE]"));
        };
        Sse::new(sse)
            .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
            .into_response()
    } else {
        // A refusal costs no LLM tokens → usage is zero.
        Json(openai::build_response(
            &copy,
            id,
            model,
            created,
            openai::Usage::default(),
        ))
        .into_response()
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
    id: &str,
    model: &str,
    created: i64,
    audit: AuditWriter,
    audit_ctx: AuditCtx,
    started: Instant,
) -> Response {
    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(INSIGHT_STREAM_BUFFER);
    let sink: Arc<dyn EventSink> = Arc::new(ChannelSink(tx));

    let (orchestrator, pipeline_id) = match build_openai_pipeline(state, report, Some(sink.clone()))
    {
        Ok(pair) => pair,
        Err(e) => {
            return openai_error(StatusCode::BAD_GATEWAY, openai::ERR_UPSTREAM, format!("{e:#}"))
        }
    };

    // The task owns the orchestrator + sink; when the run finishes every sink clone drops and the
    // channel closes, so the drain loop below ends naturally.
    let run = tokio::spawn(async move {
        orchestrator
            .run_emitting(&pipeline_id, initial, &*sink)
            .await
    });

    let mut usages: Vec<UsageData> = Vec::new();
    let mut answer: Option<String> = None;
    let mut failure: Option<String> = None;

    while let Some(event) = rx.recv().await {
        match event {
            // Accumulate per-stage token usage (D3).
            AgentEvent::Usage {
                prompt,
                completion,
                reasoning,
                total,
            } => usages.push(UsageData {
                prompt,
                completion,
                reasoning,
                total,
            }),
            // The finalizer/renderer answer is the complete result.
            AgentEvent::Finished { assistant } => answer = Some(assistant),
            AgentEvent::Error { message } => failure = Some(message),
            _ => {}
        }
    }
    // Channel closed → the run finished. A stage failure already surfaced as `AgentEvent::Error`;
    // only a task panic needs a fallback.
    if let Err(join) = run.await {
        failure.get_or_insert_with(|| format!("agent task failed: {join}"));
    }

    let duration_ms = started.elapsed().as_millis() as u64;
    match answer {
        Some(text) => {
            let usage = openai::accumulate_usage(&usages);
            if let Err(e) = audit
                .write(
                    &audit_ctx,
                    AuditEvent::ResponseCompleted {
                        response_hash: hash_identifier(&text),
                        response_chars: text.chars().count(),
                        duration_ms,
                        status: "completed".to_string(),
                    },
                )
                .await
            {
                warn!(error = %e, "chat_completions: audit ResponseCompleted failed");
            }
            Json(openai::build_response(&text, id, model, created, usage)).into_response()
        }
        None => {
            // A stage failed (ERR5) or the run aborted before `Finished` (parity with the stream
            // path: surfaced as an upstream error).
            let message = failure.unwrap_or_else(|| "pipeline produced no answer".to_string());
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
#[allow(clippy::too_many_arguments)]
fn openai_stream_response(
    state: &AppState,
    report: bool,
    initial: AgentPayload,
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

    let (orchestrator, pipeline_id) = match build_openai_pipeline(state, report, Some(sink.clone()))
    {
        Ok(pair) => pair,
        Err(e) => return openai_error(StatusCode::BAD_GATEWAY, openai::ERR_UPSTREAM, format!("{e:#}")),
    };

    // The task owns the orchestrator + sink; when the run finishes every sink clone drops and the
    // channel closes, so the drain loop below ends naturally.
    let run = tokio::spawn(async move {
        orchestrator
            .run_emitting(&pipeline_id, initial, &*sink)
            .await
    });

    let sse_stream = async_stream::stream! {
        let mut usages: Vec<UsageData> = Vec::new();
        let mut answer: Option<String> = None;
        let mut failure: Option<String> = None;

        while let Some(event) = rx.recv().await {
            match event {
                // Accumulate per-stage token usage (D3). Logged below, not placed on the OpenAI
                // wire (the `chat.completion.chunk` shape carries no usage field).
                AgentEvent::Usage {
                    prompt,
                    completion,
                    reasoning,
                    total,
                } => usages.push(UsageData {
                    prompt,
                    completion,
                    reasoning,
                    total,
                }),
                // The terminal answer is the complete result; intermediate `ContentDelta` previews
                // are intentionally ignored (D1).
                AgentEvent::Finished { assistant } => answer = Some(assistant),
                AgentEvent::Error { message } => failure = Some(message),
                _ => {}
            }
        }
        // Channel closed → the run finished. Surface a task panic as a failure.
        if let Err(join) = run.await {
            failure.get_or_insert_with(|| format!("agent task failed: {join}"));
        }

        let duration_ms = started.elapsed().as_millis() as u64;
        match answer {
            Some(text) => {
                let usage = openai::accumulate_usage(&usages);
                tracing::info!(
                    prompt_tokens = usage.prompt_tokens,
                    completion_tokens = usage.completion_tokens,
                    total_tokens = usage.total_tokens,
                    "chat_completions stream usage"
                );
                for chunk in openai::build_chunks(&text, &id, &model, created) {
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
                            response_hash: hash_identifier(&text),
                            response_chars: text.chars().count(),
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
            None => {
                // A stage failed (ERR5) or the run aborted before `Finished`. Response headers are
                // already `200` (SSE), so surface the failure in-band as an OpenAI error object,
                // then `[DONE]`.
                let message = failure.unwrap_or_else(|| "pipeline produced no answer".to_string());
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

// ──── shared prelude ───

/// Validate the current prompt contract.
fn validate_prompt(prompt: &str) -> Result<(), AppError> {
    let trimmed = prompt.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("prompt must not be empty".into()));
    }
    let char_count = prompt.chars().count();
    if char_count > USER_PROMPT_LENGTH_CAP {
        return Err(AppError::BadRequest(format!(
            "prompt exceeds {USER_PROMPT_LENGTH_CAP} chars (got {char_count})"
        )));
    }

    Ok(())
}

// ──── /insight shared helpers ───

/// Bounded buffer for the `/insight/stream` event channel.
///
/// Large enough that a normal turn's events (stage transitions plus the analyst's streamed
/// report) are never dropped by [`ChannelSink`]'s `try_send`; a lossless channel is a later
/// refinement (plan §8.2).
const INSIGHT_STREAM_BUFFER: usize = 8192;

/// Build the pipeline's `Initial` payload, stamping the turn's `now` once at this boundary
/// (plan §12.1) and carrying prior turns forward as [`Exchange`]es.
///
/// (The pipeline does not yet thread `history` into each stage's prompt — a known limitation of
/// this first cut; the payload carries it for when it does.)
fn insight_initial(req: AgentRequest) -> AgentPayload {
    let history = req
        .history
        .into_iter()
        .map(|turn| Exchange {
            user: turn.user_prompt,
            assistant: turn.model_response,
        })
        .collect();
    AgentPayload::Initial(InitialPrompt {
        prompt: req.prompt,
        history,
        now: SystemClock::default().now(),
    })
}

/// Extract the user-facing answer from the pipeline's terminal payload.
///
/// The `/insight` pipeline always ends in a `Final` (the finalizer); anything else is an internal
/// wiring fault surfaced as a host error rather than a panic.
fn final_answer(outcome: AgentPayload) -> Result<String, AppError> {
    match outcome {
        AgentPayload::Final(result) => Ok(result.assistant),
        other => Err(AppError::ServiceUnavailable(format!(
            "insight pipeline did not produce a final result (got {:?})",
            other.kind()
        ))),
    }
}

/// Map a pipeline [`AgentError`] onto the external HTTP error contract.
///
/// A capability failure (LLM transport, MCP tool) is an upstream `502`; an internal mismatch /
/// missing-artifact / unknown-tool is a wiring fault surfaced as `503`.
fn insight_error_to_app_error(err: AgentError) -> AppError {
    match err {
        AgentError::Capability(msg) => AppError::BadGateway(msg),
        other => AppError::ServiceUnavailable(other.to_string()),
    }
}

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
        AgentEvent::Error { message } => vec![StreamFrame::Error { data: message }],
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

    #[test]
    fn prompt_validation_rejects_empty_prompt() {
        let err = validate_prompt(" \n\t").expect_err("empty prompt should be rejected");
        match err {
            AppError::BadRequest(msg) => assert_eq!(msg, "prompt must not be empty"),
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn prompt_validation_preserves_existing_2000_char_cap() {
        let at_cap = "x".repeat(USER_PROMPT_LENGTH_CAP);
        assert!(validate_prompt(&at_cap).is_ok());

        let over_cap = "x".repeat(USER_PROMPT_LENGTH_CAP + 1);
        let err = validate_prompt(&over_cap).expect_err("prompt over cap should be rejected");
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("prompt exceeds 2000 chars"));
                assert!(msg.contains("got 2001"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

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
                data: "boom".into()
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
}
