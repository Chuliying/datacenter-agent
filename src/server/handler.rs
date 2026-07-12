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
use tracing::{instrument, warn, Instrument};

use rand::seq::SliceRandom;

use super::dto::{
    AgentRequest, AgentResponse, GreetingResponse, ReadyBody, ReadyChecks, StageData, StagePhase,
    StreamFrame, ToolArgsData, ToolCallData, UsageData,
};
use super::error::AppError;
use super::AppState;
use crate::agent::clock::{Clock, SystemClock};
use crate::agent::events::{AgentEvent, ChannelSink, EventSink, StageOutcome};
use crate::agent::payload::{AgentError, AgentPayload, Exchange, InitialPrompt};
use crate::agent::pipeline::{agent_pipeline_id, report_pipeline_id};
use crate::agent::wiring::{build_insight_pipeline, build_report_pipeline};

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
}
