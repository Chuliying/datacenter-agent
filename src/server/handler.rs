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
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{stream::BoxStream, Stream, StreamExt};
use tracing::{error, instrument, warn, Instrument};

use rand::seq::SliceRandom;

use super::dto::{
    AgentRequest, AgentResponse, GreetingResponse, ReadyBody, ReadyChecks, StageData, StagePhase,
    StreamFrame,
};
use super::error::AppError;
use super::AppState;
use crate::agent::clock::{Clock, SystemClock};
use crate::agent::config::{Provider, ResolvedLlm};
use crate::agent::events::{AgentEvent, ChannelSink, EventSink, StageOutcome};
use crate::agent::payload::{AgentError, AgentPayload, Exchange, InitialPrompt};
use crate::agent::pipeline::agent_pipeline_id;
use crate::agent::wiring::build_insight_pipeline;
use crate::appstate::LlmDefaults;
use crate::llm_connector;
use crate::model::GenerationConfig;

/// Upper bound on the user prompt, in UTF-8 characters.
pub const USER_PROMPT_LENGTH_CAP: usize = 2_000;

/// Output-token ceiling for the report endpoints.
///
/// A full self-contained HTML report (design-system CSS + markup + chart
/// script) far exceeds the chat-sized [`LlmDefaults::max_tokens`] default, so
/// the `/report` paths raise it. Kept below the 120 s request timeout's
/// practical generation budget.
///
/// [`LlmDefaults::max_tokens`]: crate::appstate::LlmDefaults::max_tokens
const REPORT_MAX_TOKENS: u32 = 16_384;

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
        let resolved = resolved_insight_llm(&state.llm);
        let orchestrator = build_insight_pipeline(
            state.mcp.clone(),
            &state.tools,
            state.instructions.as_deref(),
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

/// Shared completion body for the non-streaming JSON endpoints.
///
/// `base_system` selects the system prompt (analytics vs report) and
/// `max_tokens_override` bumps the output ceiling for the report path.
async fn completion_inner(
    state: AppState,
    req: AgentRequest,
    base_system: String,
    max_tokens_override: Option<u32>,
) -> Result<Json<AgentResponse>, AppError> {
    let user_prompt = req.prompt.clone();
    let cfg = prepare_config(&state, req, &base_system, max_tokens_override)?;

    // MCP tool-calling loop
    let md = match llm_connector::generate(cfg, state.tools.clone(), state.mcp.clone()).await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "completion.llm_failed");
            return Err(AppError::BadGateway(format!("{e:#}")));
        }
    };

    Ok(Json(AgentResponse {
        user_prompt,
        model_response: md,
        intent: "unknown".into(),
    }))
}

// ──── /report ───

/// HTML report handler (legacy monolith path).
///
/// Drives the `report_system` prompt through the monolith tool-loop, producing a
/// self-contained HTML report wrapped in a `falcon-report` fenced block. The
/// output ceiling is raised to `REPORT_MAX_TOKENS` to fit a full document.
pub async fn report(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<AgentResponse>, AppError> {
    let Json(req) = req?;

    // Prepare logging
    let span = tracing::info_span!(
        "report",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
    );
    let base_system = state.prompts.report_system.clone();
    completion_inner(state, req, base_system, Some(REPORT_MAX_TOKENS))
        .instrument(span)
        .await
}

// ──── /report/stream ───

/// Server-Sent Events variant of [`report`].
///
/// Same `data:`-JSON-envelope wire contract as [`insight_stream`]; the `falcon-report` HTML block
/// streams token-by-token like any other answer.
pub async fn report_stream(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let Json(req) = req?;

    // Prepare logging
    let span = tracing::info_span!(
        "report-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
    );
    let _enter = span.enter();

    // Prepare configuration
    let cfg = prepare_config(
        &state,
        req,
        &state.prompts.report_system,
        Some(REPORT_MAX_TOKENS),
    )?;

    // Build SSE stream
    let sse_stream: BoxStream<'static, Result<Event, Infallible>> =
        llm_connector::agent_stream(cfg, state.tools.clone(), state.mcp.clone())
            .filter_map(|ev| async move {
                let frame = stream_frame_from_llm_event(ev)?;
                let event = Event::default()
                    .json_data(&frame)
                    .expect("unexpected error: StreamFrame is always valid JSON");
                Some(Ok::<_, Infallible>(event))
            })
            .boxed();

    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE)))
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

    let resolved = resolved_insight_llm(&state.llm);
    let orchestrator = build_insight_pipeline(
        state.mcp.clone(),
        &state.tools,
        state.instructions.as_deref(),
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

/// Serialize a stream frame into an SSE event.
fn sse_event(frame: StreamFrame) -> Event {
    Event::default()
        .json_data(&frame)
        .expect("unexpected error: StreamFrame is always valid JSON")
}

// ──── shared prelude ───

/// Prepare a [`GenerationConfig`] from the request.
///
/// Validate the request and assemble a [`GenerationConfig`] for the MCP
/// tool-calling loop. `base_system` selects which system prompt drives the
/// loop; `max_tokens_override` (when `Some`) raises the output ceiling above
/// [`LlmDefaults`] for the report path.
///
/// Shared by the legacy monolith `/report` and `/report/stream` paths. (The analytics endpoints
/// are now `/insight` + `/insight/stream`, which drive the sub-agent pipeline instead.)
///
/// [`LlmDefaults`]: crate::appstate::LlmDefaults
fn prepare_config(
    state: &AppState,
    req: AgentRequest,
    base_system: &str,
    max_tokens_override: Option<u32>,
) -> Result<GenerationConfig, AppError> {
    validate_prompt(&req.prompt)?;

    // build config
    let mut cfg = state.generation_config(base_system, req.prompt, req.history);
    if let Some(max_tokens) = max_tokens_override {
        cfg.max_tokens = max_tokens;
    }
    Ok(cfg)
}

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

/// Map internal LLM stream events to the stable external SSE frame contract.
fn stream_frame_from_llm_event(ev: llm_connector::LlmEvent) -> Option<StreamFrame> {
    match ev {
        llm_connector::LlmEvent::Token(t) => Some(StreamFrame::Token { data: t }),
        llm_connector::LlmEvent::Done => Some(StreamFrame::Done),
        llm_connector::LlmEvent::Error(e) => Some(StreamFrame::Error { data: e }),
        llm_connector::LlmEvent::Clear => Some(StreamFrame::Clear),
        llm_connector::LlmEvent::ToolCalled { .. } | llm_connector::LlmEvent::ToolResult { .. } => {
            None
        }
    }
}

// ──── /insight shared helpers ───

/// Bounded buffer for the `/insight/stream` event channel.
///
/// Large enough that a normal turn's events (stage transitions plus the analyst's streamed
/// report) are never dropped by [`ChannelSink`]'s `try_send`; a lossless channel is a later
/// refinement (plan §8.2).
const INSIGHT_STREAM_BUFFER: usize = 8192;

/// Bridge the environment's [`LlmDefaults`] onto the sub-agent layer's [`ResolvedLlm`].
///
/// Every `/insight` stage runs on OpenRouter (the deployment's single provider today) with the
/// shared model and sampling params.
fn resolved_insight_llm(llm: &LlmDefaults) -> ResolvedLlm {
    ResolvedLlm {
        provider: Provider::OpenRouter,
        base_url: llm.base_url.clone(),
        model: llm.model.clone(),
        temperature: llm.temperature,
        top_p: llm.top_p,
        max_tokens: llm.max_tokens,
        api_key: Some(llm.api_key.clone()),
    }
}

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
/// live as `token`s (delimited by those stage frames). On the terminal `Finished` the
/// **complete** finalizer answer (report + `falcon-chart` blocks) is re-sent after a `clear`, so a
/// consumer always ends with the correct full answer regardless of the intermediate token
/// previews. The finer-grained tool / reasoning events stay internal for now.
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
        AgentEvent::Finished { assistant } => vec![
            StreamFrame::Clear,
            StreamFrame::Token { data: assistant },
            StreamFrame::Done,
        ],
        AgentEvent::Error { message } => vec![StreamFrame::Error { data: message }],
        // Internal framing (stage produced, tool + reasoning deltas, proposed calls) is not
        // surfaced to the browser yet.
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
    fn stream_mapping_preserves_external_sse_events() {
        assert_eq!(
            stream_frame_from_llm_event(llm_connector::LlmEvent::Token("hi".into())),
            Some(StreamFrame::Token { data: "hi".into() })
        );
        assert_eq!(
            stream_frame_from_llm_event(llm_connector::LlmEvent::Error("boom".into())),
            Some(StreamFrame::Error {
                data: "boom".into()
            })
        );
        assert_eq!(
            stream_frame_from_llm_event(llm_connector::LlmEvent::Clear),
            Some(StreamFrame::Clear)
        );
        assert_eq!(
            stream_frame_from_llm_event(llm_connector::LlmEvent::Done),
            Some(StreamFrame::Done)
        );
        assert_eq!(
            stream_frame_from_llm_event(llm_connector::LlmEvent::ToolCalled {
                name: "tool".into(),
                args_hash: "hash".into()
            }),
            None
        );
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
}
