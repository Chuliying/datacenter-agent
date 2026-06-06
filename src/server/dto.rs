//! Data structures for the HTTP endpoints.

use serde::{Deserialize, Serialize};

use crate::model::History;

// ──── /agent ────

/// Request body for `POST /agent`.
///
/// `history` is optional (defaults to empty) so the very first turn of a
/// conversation doesn't have to send `"history": []`.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentRequest {
    #[serde(default)]
    pub history: Vec<History>,
    pub prompt: String,
}

/// A success response for `POST /agent`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentResponse {
    pub user_prompt: String,
    pub model_response: String,
}

/// One frame on the `POST /agent/stream` SSE wire.
///
/// Every frame is a JSON object inside a single `data:` line, with `event`
/// as the discriminator.
///
/// - `token`: The `data` field carries the token text.
/// - `error`: The `data` field carries the error message.
/// - `done`: Carries no payload, used to indicate the end of the stream.
/// - `clear`: Carries no payload, used to suggest down stream reset current accumulated tokens.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum StreamFrame {
    /// Token event, used to stream tokens
    Token { data: String },
    /// Done event, used to indicate the end of the stream
    Done,
    /// Error event, used to indicate an error
    Error { data: String },
    /// Clear event, used to suggest down stream reset current accumulated tokens.
    Clear,
}

// ──── /greeting ───

/// Success response for `GET /greeting`.
///
/// This is one random pre-generated greeting paragraph from
/// [`crate::server::state::AppState::greetings`].
#[derive(Debug, Clone, Serialize)]
pub struct GreetingResponse {
    pub greeting: String,
}

// ──── /ready ───

// Readiness probe data type

#[derive(Debug, Clone, Serialize)]
pub struct ReadyChecks {
    pub api_key: bool,
    pub base_url_reachable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReadyBody {
    pub ready: bool,
    pub checks: ReadyChecks,
}
