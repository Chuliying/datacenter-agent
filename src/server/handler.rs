//! Request handlers.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use axum::Json;
use futures::{Stream, StreamExt};
use tracing::{error, instrument, warn, Instrument};

use rand::seq::SliceRandom;

use super::dto::{
    AgentRequest, AgentResponse, GreetingResponse, ReadyBody, ReadyChecks, StreamFrame,
};
use super::error::AppError;
use super::state::AppState;
use crate::llm_connector;
use crate::model::GenerationConfig;

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
    );
    agent_inner(state, req).instrument(span).await
}

async fn agent_inner(state: AppState, req: AgentRequest) -> Result<Json<AgentResponse>, AppError> {
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
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let Json(req) = req?;

    // Prepare logging
    let span = tracing::info_span!(
        "agent-stream",
        prompt_len = req.prompt.chars().count(),
        history_len = req.history.len(),
    );
    let _enter = span.enter();

    // Prepare configuration
    let cfg = prepare_config(&state, req)?;

    // Build SSE stream
    let sse_stream =
        llm_connector::agent_stream(cfg, state.tools.clone(), state.mcp.clone()).map(|ev| {
            let frame = match ev {
                llm_connector::LlmEvent::Token(t) => StreamFrame::Token { data: t },
                llm_connector::LlmEvent::Done => StreamFrame::Done,
                llm_connector::LlmEvent::Error(e) => StreamFrame::Error { data: e },
                llm_connector::LlmEvent::Clear => StreamFrame::Clear,
            };
            let event = Event::default()
                .json_data(&frame)
                .expect("unexpected error: StreamFrame is always valid JSON");
            Ok::<_, Infallible>(event)
        });

    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE)))
}

// ──── shared prelude ───

/// Prepare a [`GenerationConfig`] from the request.
///
/// Validate the request and assemble a [`GenerationConfig`] for the MCP
/// tool-calling loop.
///
/// Shared by both `/agent` and `/agent/stream`.
fn prepare_config(state: &AppState, req: AgentRequest) -> Result<GenerationConfig, AppError> {
    // validate
    let trimmed = req.prompt.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest("prompt must not be empty".into()));
    }
    let char_count = req.prompt.chars().count();
    if char_count > USER_PROMPT_LENGTH_CAP {
        return Err(AppError::BadRequest(format!(
            "prompt exceeds {USER_PROMPT_LENGTH_CAP} chars (got {char_count})"
        )));
    }

    // build config
    Ok(state.generation_config(&state.prompts.agent_system, req.prompt, req.history))
}
