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
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub option_id: Option<String>,
}

/// A success response for `POST /agent`.
#[derive(Debug, Clone, Serialize)]
pub struct AgentResponse {
    pub user_prompt: String,
    pub model_response: String,
    /// Intent selected by the runtime input pipeline.
    ///
    /// `"unknown"` when the runtime is disabled (legacy loop) or the turn was
    /// refused/aborted before an intent could be resolved.
    pub intent: String,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    /// Intent resolved event, emitted once before any token so the host can
    /// pick the answer topic branch. Mirrors the frontend `intent.resolved`
    /// event shape (`data: { intent, candidateIntents }`).
    #[serde(rename = "intent.resolved")]
    IntentResolved { data: IntentResolvedData },
}

/// Payload for the [`StreamFrame::IntentResolved`] event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentResolvedData {
    /// Resolved intent.
    pub intent: String,
    /// Candidate intents considered by the pipeline.
    pub candidate_intents: Vec<String>,
}

// ──── /greeting ───

/// Success response for `GET /greeting`.
///
/// This is one random pre-generated greeting paragraph from
/// [`crate::appstate::AppState::greetings`].
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
