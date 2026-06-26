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
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::{stream::BoxStream, StreamExt};
use tracing::{error, instrument, warn, Instrument};
use uuid::Uuid;

use rand::seq::SliceRandom;

use super::dto::{
    AgentRequest, AgentResponse, GreetingResponse, ReadyBody, ReadyChecks, StreamFrame,
};
use super::error::AppError;
use super::AppState;
use crate::appstate::AppRuntime;
use crate::llm_connector;
use crate::model::GenerationConfig;
use crate::runtime::audit::{AuditCtx, AuditWriter};
use crate::runtime::orchestrator::{run_agent_turn, AgentTurnDeps, AgentTurnOutcome, LlmAgentPort};
use crate::runtime::schema::AgentTurnInput;

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

// ──── /agent ───

/// Regular agent chat handler.
pub async fn agent(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<AgentResponse>, AppError> {
    let Json(req) = req?;

    // Prepare logging
    let span = tracing::info_span!(
        "agent",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
        session_id = req.session_id.as_deref().unwrap_or(""),
        option_id = req.option_id.as_deref().unwrap_or(""),
    );
    agent_inner(state, req).instrument(span).await
}

async fn agent_inner(state: AppState, req: AgentRequest) -> Result<Json<AgentResponse>, AppError> {
    if should_use_runtime(state.runtime.as_deref()) {
        return agent_inner_runtime(state, req).await;
    }

    let user_prompt = req.prompt.clone();
    let cfg = prepare_config(&state, req)?;

    // MCP tool-calling loop
    let md = match llm_connector::generate(cfg, state.tools.clone(), state.mcp.clone()).await {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "agent.llm_failed");
            return Err(AppError::BadGateway(format!("{e:#}")));
        }
    };

    Ok(Json(AgentResponse {
        user_prompt,
        model_response: md,
    }))
}

async fn agent_inner_runtime(
    state: AppState,
    req: AgentRequest,
) -> Result<Json<AgentResponse>, AppError> {
    let runtime = state
        .runtime
        .clone()
        .ok_or_else(|| AppError::ServiceUnavailable("runtime not configured".into()))?;
    let user_prompt = req.prompt.clone();
    let cfg = state.generation_config(
        &state.prompts.agent_system,
        req.prompt.clone(),
        req.history.clone(),
    );
    let agent = LlmAgentPort::new(cfg, state.tools.clone(), state.mcp.clone());
    let audit = AuditWriter::new(runtime.audit_sink.clone(), runtime.audit_failure_policy);
    let request_id = Uuid::new_v4();
    let audit_ctx = AuditCtx {
        request_id: request_id.to_string(),
        session_id: req.session_id.clone(),
        route: "/agent".into(),
        actor: None,
    };
    let turn_input = AgentTurnInput {
        request_id,
        prompt: req.prompt,
        history: req.history,
        session_id: req.session_id,
        option_id: req.option_id,
    };

    let outcome = run_agent_turn(
        turn_input,
        &audit_ctx,
        AgentTurnDeps {
            runtime_config: &runtime.config,
            input_pipeline: &runtime.input_pipeline,
            answer_policy: runtime.answer_policy.as_ref(),
            llm_normalizer: runtime.llm_normalizer.as_deref(),
            sessions: runtime.sessions.as_deref(),
            agent: &agent,
            audit: &audit,
        },
    )
    .await
    .map_err(runtime_error_to_app_error)?;

    match outcome {
        AgentTurnOutcome::Final { response, .. }
        | AgentTurnOutcome::Aborted { response }
        | AgentTurnOutcome::Refused { copy: response, .. } => Ok(Json(AgentResponse {
            user_prompt,
            model_response: response,
        })),
        AgentTurnOutcome::Error { code, status } => Err(runtime_outcome_error(code, status)),
    }
}

// ──── /agent/stream ───

/// Server-Sent Events variant of [`agent`].
///
/// Every frame is a single SSE `data:` line carrying a JSON envelope:
/// - `{"event":"token","data":"<text fragment>"}`: append to the answer.
/// - `{"event":"done"}`: model finished cleanly; close the connection.
/// - `{"event":"error","data":"<message>"}`: terminal error; close the connection.
/// - `{"event":"clear"}`: suggest to clear the answer.
pub async fn agent_stream(
    State(state): State<AppState>,
    req: Result<Json<AgentRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let Json(req) = req?;

    // Prepare logging
    let span = tracing::info_span!(
        "agent-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
        session_id = req.session_id.as_deref().unwrap_or(""),
        option_id = req.option_id.as_deref().unwrap_or(""),
    );
    let _enter = span.enter();

    if should_use_runtime(state.runtime.as_deref()) {
        return agent_stream_runtime(state, req).await;
    }

    // Prepare configuration
    let cfg = prepare_config(&state, req)?;

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

    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
        .into_response())
}

async fn agent_stream_runtime(state: AppState, req: AgentRequest) -> Result<Response, AppError> {
    let response = agent_inner_runtime(state, req).await?.0;
    let frames = vec![
        StreamFrame::Token {
            data: response.model_response,
        },
        StreamFrame::Done,
    ];
    let sse_stream: BoxStream<'static, Result<Event, Infallible>> = futures::stream::iter(frames)
        .map(|frame| {
            let event = Event::default()
                .json_data(&frame)
                .expect("unexpected error: StreamFrame is always valid JSON");
            Ok::<_, Infallible>(event)
        })
        .boxed();
    Ok(Sse::new(sse_stream)
        .keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
        .into_response())
}

// ──── shared prelude ───

/// Prepare a [`GenerationConfig`] from the request.
///
/// Validate the request and assemble a [`GenerationConfig`] for the MCP
/// tool-calling loop.
///
/// Shared by both `/agent` and `/agent/stream`.
fn prepare_config(state: &AppState, req: AgentRequest) -> Result<GenerationConfig, AppError> {
    validate_prompt(&req.prompt)?;

    // build config
    Ok(state.generation_config(&state.prompts.agent_system, req.prompt, req.history))
}

/// Validate the current `/agent` prompt contract.
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

fn should_use_runtime(runtime: Option<&AppRuntime>) -> bool {
    runtime.is_some_and(|runtime| runtime.enabled)
}

fn runtime_error_to_app_error(err: crate::runtime::error::RuntimeError) -> AppError {
    match err {
        crate::runtime::error::RuntimeError::InputRequired
        | crate::runtime::error::RuntimeError::InputTooLong(_) => {
            AppError::BadRequest(err.to_string())
        }
        crate::runtime::error::RuntimeError::Upstream(_) => AppError::BadGateway(err.to_string()),
        crate::runtime::error::RuntimeError::AuditSink(_)
        | crate::runtime::error::RuntimeError::Config(_)
        | crate::runtime::error::RuntimeError::UnknownModule { .. }
        | crate::runtime::error::RuntimeError::IntentNotAllowed(_)
        | crate::runtime::error::RuntimeError::PipelineContract
        | crate::runtime::error::RuntimeError::Internal(_)
        | crate::runtime::error::RuntimeError::Request(_) => {
            AppError::ServiceUnavailable(err.to_string())
        }
    }
}

fn runtime_outcome_error(code: String, status: u16) -> AppError {
    match status {
        400 => AppError::BadRequest(code),
        502 => AppError::BadGateway(code),
        _ => AppError::ServiceUnavailable(code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::config::AppConfig;
    use crate::runtime::audit::AuditFailurePolicy;
    use crate::runtime::config::RuntimeConfig;
    use crate::runtime::input::pipeline::InputPipeline;
    use crate::runtime::registry::BuiltinRegistry;

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
    fn runtime_route_selection_is_default_off_and_flagged_on() {
        assert!(!should_use_runtime(None));

        let disabled = app_runtime(false);
        assert!(!should_use_runtime(Some(&disabled)));

        let enabled = app_runtime(true);
        assert!(should_use_runtime(Some(&enabled)));
    }

    fn app_runtime(enabled: bool) -> AppRuntime {
        let app_config = AppConfig::load("config/config.toml").expect("app config should load");
        let refs = app_config
            .runtime
            .expect("runtime refs should be configured");
        let registry = BuiltinRegistry::default();
        let config = RuntimeConfig::load(&refs, &registry).expect("runtime config should load");
        let answer_policy = registry
            .build_answer_policy(&config)
            .expect("answer policy should build");
        let llm_normalizer = registry
            .build_llm_normalizer(&config)
            .expect("LLM normalizer should build");
        let sessions = registry.build_memory(&config).expect("memory should build");
        let audit_sink = registry.build_audit(&config).expect("audit should build");

        AppRuntime {
            enabled,
            config: Arc::new(config),
            input_pipeline: InputPipeline::default(),
            answer_policy,
            llm_normalizer,
            sessions,
            audit_sink,
            audit_failure_policy: AuditFailurePolicy::FailOpen,
        }
    }
}
