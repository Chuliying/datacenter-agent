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

/// One frame on the `POST /insight/stream` (and legacy `/report/stream`) SSE wire.
///
/// Every frame is a JSON object inside a single `data:` line, with `event`
/// as the discriminator.
///
/// - `token`: The `data` field carries the token text.
/// - `error`: The `data` field carries the error message.
/// - `done`: Carries no payload, used to indicate the end of the stream.
/// - `clear`: Carries no payload, used to suggest down stream reset current accumulated tokens.
/// - `stage`: The `data` field names the sub-agent now running (the pipeline streams
///   `/insight/stream` + `/report/stream`).
/// - `tool_call`: The `data` field names a tool call the model proposed (pipeline streams).
/// - `tool_args`: The `data` field carries a fragment of a tool call's streamed arguments — live
///   progress while the model composes a call (pipeline streams).
/// - `usage`: The `data` field carries one turn's token counts, incl. hidden reasoning tokens
///   (pipeline streams).
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
    /// Sub-agent stage transition. `data` carries the sub-agent id and its lifecycle phase
    /// (`started`, then `success` / `failure` on completion) — enough for a client to show a
    /// per-stage progress indicator that turns green or red. Emitted by the pipeline streams
    /// (`/insight/stream`, `/report/stream`).
    Stage { data: StageData },
    /// A tool call the model proposed (its arguments are assembled). `data` carries the call id and
    /// the tool name, so a client can label the call whose arguments it was streaming. Emitted by
    /// the pipeline streams.
    #[serde(rename = "tool_call")]
    ToolCall { data: ToolCallData },
    /// A fragment of a tool call's streamed JSON arguments — live progress while the model composes
    /// a call, so a long tool-calling turn keeps showing the task is still running. `data` carries
    /// the call id and the raw argument fragment. Emitted by the pipeline streams.
    #[serde(rename = "tool_args")]
    ToolArgs { data: ToolArgsData },
    /// Token usage for one completed LLM turn (a stage may report several). `data` carries the
    /// prompt / completion / total counts plus the reasoning-token subset when the model reports it
    /// — the "hidden" tokens behind a silent burn. Emitted by the pipeline streams.
    Usage { data: UsageData },
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

/// Payload for the [`StreamFrame::Stage`] event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StageData {
    /// The sub-agent this transition is about (e.g. `fetcher` / `analyst` / `charter` /
    /// `finalizer` for `/insight`; `fetcher` / `analyst` / `composer` / `renderer` for `/report`).
    pub agent: String,
    /// Whether the sub-agent just started, or finished with success / failure.
    pub phase: StagePhase,
}

/// Lifecycle phase of a sub-agent stage, for a start → success/failure indicator (e.g. a dot
/// that spins, then turns green or red).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StagePhase {
    /// The stage began running.
    Started,
    /// The stage finished successfully (a green dot).
    Success,
    /// The stage finished with an error (a red dot); a terminal `error` frame follows.
    Failure,
}

/// Payload for the [`StreamFrame::ToolCall`] event — a proposed tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCallData {
    /// The tool-call id, correlating this call with its `tool_args` fragments.
    pub id: String,
    /// The advertised tool name the model chose to call.
    pub name: String,
}

/// Payload for the [`StreamFrame::ToolArgs`] event — one streamed argument fragment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolArgsData {
    /// The tool-call id this fragment belongs to (matches a later `tool_call` frame's `id`).
    pub id: String,
    /// A raw fragment of the tool call's JSON arguments (partial; concatenate by `id`).
    pub fragment: String,
}

/// Payload for the [`StreamFrame::Usage`] event — one turn's token accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct UsageData {
    /// Prompt (input) tokens.
    pub prompt: u32,
    /// Completion (output) tokens — includes `reasoning`.
    pub completion: u32,
    /// Reasoning tokens (a subset of `completion`), when the model reports them; `null` otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<u32>,
    /// Total tokens (`prompt` + `completion`).
    pub total: u32,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The SSE wire is an external contract the frontend parses by its `event` discriminator, so
    /// pin the exact JSON shape of the tool-call progress frames.
    #[test]
    fn tool_progress_frames_serialize_to_their_wire_shape() {
        let call = StreamFrame::ToolCall {
            data: ToolCallData {
                id: "call_1".into(),
                name: "bill_revenue".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(&call).unwrap(),
            serde_json::json!({
                "event": "tool_call",
                "data": { "id": "call_1", "name": "bill_revenue" }
            })
        );

        let args = StreamFrame::ToolArgs {
            data: ToolArgsData {
                id: "call_1".into(),
                fragment: "{\"seller_id\":".into(),
            },
        };
        assert_eq!(
            serde_json::to_value(&args).unwrap(),
            serde_json::json!({
                "event": "tool_args",
                "data": { "id": "call_1", "fragment": "{\"seller_id\":" }
            })
        );
    }

    #[test]
    fn usage_frame_serializes_with_and_without_reasoning() {
        // With reasoning tokens (a reasoning model): the breakdown is present.
        let with = StreamFrame::Usage {
            data: UsageData {
                prompt: 1200,
                completion: 8000,
                reasoning: Some(7600),
                total: 9200,
            },
        };
        assert_eq!(
            serde_json::to_value(&with).unwrap(),
            serde_json::json!({
                "event": "usage",
                "data": { "prompt": 1200, "completion": 8000, "reasoning": 7600, "total": 9200 }
            })
        );

        // Without: `reasoning` is omitted from the wire (not sent as null).
        let without = StreamFrame::Usage {
            data: UsageData {
                prompt: 100,
                completion: 50,
                reasoning: None,
                total: 150,
            },
        };
        assert_eq!(
            serde_json::to_value(&without).unwrap(),
            serde_json::json!({
                "event": "usage",
                "data": { "prompt": 100, "completion": 50, "total": 150 }
            })
        );
    }
}
