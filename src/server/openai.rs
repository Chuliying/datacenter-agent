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

//! OpenAI-compatible `POST /v1/chat/completions` DTOs and mapping.
//!
//! This endpoint lets the service be registered as an agentgateway Path C
//! (OpenAI-compatible LLM backend) while `/agent/stream` and its rich SSE
//! events stay unchanged. The wire shape here is standard OpenAI; internally we
//! map onto the existing [`AgentRequest`] and drive the same runtime prelude +
//! sub-agent pipeline. See `docs/work/agentgateway-openai-endpoint/spec.md`.

use serde::{Deserialize, Deserializer, Serialize};

use crate::model::History;
use crate::server::dto::{AgentRequest, UsageData};

/// One OpenAI chat message (`{role, content}`).
///
/// Deserialized from the request `messages`, and reused as the `assistant`
/// message in a non-streaming [`ChatCompletionResponse`] — hence `Serialize`.
///
/// `content` accepts both OpenAI shapes: a plain string, **or** an array of typed content parts
/// (`[{"type":"text","text":"…"}, …]`). For the array form the text parts are concatenated and any
/// non-text part (e.g. `image_url`) is ignored — this endpoint is text-only. On the response side
/// `content` always serializes as a plain string (the OpenAI response shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(deserialize_with = "deserialize_content")]
    pub content: String,
}

/// Deserialize an OpenAI message `content` from either a plain string or a content-parts array.
///
/// A string passes through unchanged. An array is flattened to the concatenation of its `text`
/// parts (parts of any other `type` are dropped); this keeps a real OpenAI SDK client — which sends
/// `content` as parts — working, while the pipeline only consumes text.
fn deserialize_content<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    /// One typed part of a content-parts array. Only `text` parts carry text we keep.
    #[derive(Deserialize)]
    struct ContentPart {
        #[serde(rename = "type")]
        kind: String,
        #[serde(default)]
        text: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Content {
        Text(String),
        Parts(Vec<ContentPart>),
    }

    Ok(match Content::deserialize(deserializer)? {
        Content::Text(text) => text,
        Content::Parts(parts) => parts
            .into_iter()
            .filter(|part| part.kind == "text")
            .filter_map(|part| part.text)
            .collect::<String>(),
    })
}

/// Request body for `POST /v1/chat/completions` (OpenAI Chat Completions shape).
///
/// Only the fields this endpoint honors are deserialized; unknown fields are ignored (serde's
/// default). In particular OpenAI's `tools` / `tool_choice` are **deliberately not** captured: this
/// endpoint fronts a fixed internal sub-agent pipeline and does not expose client-driven
/// tool-calling, so advertising or selecting tools has no effect here. A client that sends them is
/// accepted (they are silently dropped) rather than rejected.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
    /// OpenAI `stream_options`; absent for a client that does not ask for a usage chunk.
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
}

/// OpenAI `stream_options`. Only `include_usage` is honored — the sole option that affects this
/// endpoint's wire (a terminal usage-only chunk, see [`usage_chunk`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct StreamOptions {
    /// When `true`, emit a final usage-only chunk before `data: [DONE]`.
    #[serde(default)]
    pub include_usage: bool,
}

/// Why an OpenAI `messages` list could not be mapped onto an [`AgentRequest`].
///
/// All variants surface to the client as HTTP 400 `invalid_request_error`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MapError {
    /// No messages, or no `user` message to use as the prompt.
    NoUserMessage,
    /// The conversation has messages but no trailing `user` turn to serve as the prompt
    /// (e.g. it ends with an `assistant` turn).
    BadShape(String),
}

/// Whether a message role is dropped before shaping the conversation. Only `user` / `assistant`
/// carry a conversational turn; every other role is ignored rather than folded into one — `system`
/// / `developer` (the pipeline has no system slot, each stage carries its own designed
/// instruction), and any other role such as `tool` / `function` (this endpoint advertises no
/// tools). Dropping unknown roles means replaying a transcript that carries tool messages never
/// pollutes the user/assistant history.
fn is_ignored_role(role: &str) -> bool {
    role != "user" && role != "assistant"
}

/// Map an OpenAI `messages` list onto the internal [`AgentRequest`].
///
/// The mapping is lenient about turn structure so a real OpenAI client is not rejected for a
/// non-strictly-alternating history:
///
/// - Non-conversational roles (`system` / `developer` / `tool` / …) are dropped (see [`is_ignored_role`]).
/// - Consecutive messages of the **same** role are merged into one turn, their content joined with
///   `\n` (two `user` messages in a row become one; likewise `assistant`).
/// - After merging, the trailing `user` turn becomes `prompt`; the earlier turns fold into
///   `history`. A conversation that opens with an `assistant` turn pairs it with an empty
///   `user_prompt` rather than failing.
/// - There must still be a trailing `user` turn to serve as the prompt: an empty list (or one made
///   empty by dropping system/developer) is [`MapError::NoUserMessage`]; a list that ends on an
///   `assistant` turn is [`MapError::BadShape`]. Both surface as HTTP 400.
///
/// `session_id` / `option_id` have no OpenAI equivalent and are left `None`.
pub fn map_request(messages: Vec<ChatMessage>) -> Result<AgentRequest, MapError> {
    // Drop system/developer, then collapse runs of the same role into one turn so the remainder is
    // strictly alternating regardless of how the client batched its messages.
    let mut merged: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for msg in messages.into_iter().filter(|m| !is_ignored_role(&m.role)) {
        match merged.last_mut() {
            Some(prev) if prev.role == msg.role => {
                prev.content.push('\n');
                prev.content.push_str(&msg.content);
            }
            _ => merged.push(msg),
        }
    }

    // The current user turn (the last merged message) is the prompt.
    let Some((last, earlier)) = merged.split_last() else {
        return Err(MapError::NoUserMessage);
    };
    if last.role != "user" {
        return Err(MapError::BadShape(format!(
            "conversation must end with a `user` turn to use as the prompt, got role={}",
            last.role
        )));
    }
    let prompt = last.content.clone();

    // Fold the earlier (already-alternating) turns into history pairs. A leading `assistant` turn
    // pairs with an empty `user_prompt`; every `user` turn pairs with the `assistant` turn that
    // follows it (or an empty response if none does).
    let mut history = Vec::with_capacity(earlier.len().div_ceil(2));
    let mut i = 0;
    while i < earlier.len() {
        let turn = &earlier[i];
        if turn.role == "assistant" {
            history.push(History {
                user_prompt: String::new(),
                model_response: turn.content.clone(),
            });
            i += 1;
        } else {
            let model_response = match earlier.get(i + 1) {
                Some(next) if next.role == "assistant" => {
                    i += 2;
                    next.content.clone()
                }
                _ => {
                    i += 1;
                    String::new()
                }
            };
            history.push(History {
                user_prompt: turn.content.clone(),
                model_response,
            });
        }
    }

    Ok(AgentRequest {
        history,
        prompt,
        session_id: None,
        option_id: None,
    })
}

// ──── OpenAI response / streaming DTOs ────

/// OpenAI `usage` object. Reasoning tokens (when present) sit under
/// `completion_tokens_details`, matching the OpenAI schema.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<ReasoningDetails>,
}

/// The `completion_tokens_details.reasoning_tokens` sub-object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReasoningDetails {
    pub reasoning_tokens: u32,
}

/// One streaming chunk (`object: "chat.completion.chunk"`).
///
/// `usage` is `None` (and serde-skipped) on every ordinary chunk, so the streamed wire shape is
/// unchanged; it is populated only on the terminal usage-only chunk built by [`usage_chunk`] when
/// the client sets `stream_options.include_usage` (OpenAI semantics).
///
/// (No `Eq`: [`ChunkChoice::logprobs`] holds a `serde_json::Value`, which is not `Eq`.)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatCompletionChunk {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    /// OpenAI `stream_options.include_usage`, encoded in three states via `Option<Option<_>>`:
    /// `None` → the field is **omitted** (the client did not opt in; the streamed wire is unchanged);
    /// `Some(None)` → serialized as explicit **`null`** (opted in — every content chunk carries
    /// `usage: null`); `Some(Some(_))` → the real total on the terminal usage-only chunk from
    /// [`usage_chunk`]. The single `skip_serializing_if` distinguishes "absent" from the two
    /// present forms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Option<Usage>>,
    /// OpenAI `system_fingerprint`. We compute none, so it serializes as `null` (present, matching
    /// the OpenAI envelope) rather than being omitted.
    pub system_fingerprint: Option<String>,
}

/// One choice inside a streaming chunk.
///
/// (No `Eq`: `logprobs` holds a `serde_json::Value`, which is not `Eq`.)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// OpenAI `logprobs`; always `null` on this endpoint (logprobs are not produced), serialized
    /// rather than skipped so the choice shape matches OpenAI.
    pub logprobs: Option<serde_json::Value>,
}

/// The incremental `delta` of a streaming chunk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Characters per streamed content chunk (pseudo-streaming, spec D1).
const CHUNK_CHARS: usize = 24;

/// Sum per-stage [`UsageData`] into a single OpenAI [`Usage`] (spec D3).
///
/// The pipeline emits one `Usage` per stage/LLM turn and never aggregates them;
/// this rolls them up. `reasoning` stays `None` unless at least one stage
/// reported reasoning tokens.
pub fn accumulate_usage(usages: &[UsageData]) -> Usage {
    let mut out = Usage::default();
    let mut reasoning: Option<u32> = None;
    for u in usages {
        out.prompt_tokens += u.prompt;
        out.completion_tokens += u.completion;
        out.total_tokens += u.total;
        if let Some(r) = u.reasoning {
            reasoning = Some(reasoning.unwrap_or(0) + r);
        }
    }
    out.completion_tokens_details = reasoning.map(|r| ReasoningDetails { reasoning_tokens: r });
    out
}

/// Split a complete answer into an OpenAI chunk sequence (pseudo-streaming, D1):
/// a leading `role` chunk, content chunks whose concatenation equals `answer`,
/// then a terminal `finish_reason: "stop"` chunk.
pub fn build_chunks(
    answer: &str,
    id: &str,
    model: &str,
    created: i64,
    include_usage: bool,
) -> Vec<ChatCompletionChunk> {
    let mk = |choices: Vec<ChunkChoice>| ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created,
        model: model.to_string(),
        choices,
        // OpenAI `include_usage`: opted-in content chunks carry an explicit `usage: null`
        // (`Some(None)`); otherwise the field is omitted (`None`). The real total rides only on the
        // terminal usage-only chunk (`usage_chunk`).
        usage: if include_usage { Some(None) } else { None },
        system_fingerprint: None,
    };
    // Leading chunk announces the assistant role.
    let mut chunks = vec![mk(vec![ChunkChoice {
        index: 0,
        delta: Delta {
            role: Some("assistant".into()),
            content: None,
        },
        finish_reason: None,
        logprobs: None,
    }])];
    // Content chunks — char-safe split so multibyte UTF-8 is never cut mid-codepoint.
    let chars: Vec<char> = answer.chars().collect();
    for piece in chars.chunks(CHUNK_CHARS) {
        chunks.push(mk(vec![ChunkChoice {
            index: 0,
            delta: Delta {
                role: None,
                content: Some(piece.iter().collect()),
            },
            finish_reason: None,
            logprobs: None,
        }]));
    }
    // Terminal chunk carries the stop reason and an empty delta.
    chunks.push(mk(vec![ChunkChoice {
        index: 0,
        delta: Delta::default(),
        finish_reason: Some("stop".into()),
        logprobs: None,
    }]));
    chunks
}

/// Build the terminal usage-only chunk for OpenAI `stream_options.include_usage` (spec D3).
///
/// Its `choices` is empty and `usage` carries the accumulated per-stage total. It is sent after the
/// content chunks from [`build_chunks`] and before `data: [DONE]`; only when the client opted in
/// (otherwise the stream is byte-for-byte what it was before this field existed).
pub fn usage_chunk(usage: Usage, id: &str, model: &str, created: i64) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created,
        model: model.to_string(),
        choices: Vec::new(),
        usage: Some(Some(usage)),
        system_fingerprint: None,
    }
}

// ──── non-streaming response DTO (spec D2) ────

/// OpenAI `chat.completion` response body (`stream=false`, spec D2).
///
/// (No `Eq`: [`Choice::logprobs`] holds a `serde_json::Value`, which is not `Eq`.)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: i64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
    /// OpenAI `system_fingerprint`. We compute none, so it serializes as `null` (present, matching
    /// the OpenAI envelope) rather than being omitted.
    pub system_fingerprint: Option<String>,
}

/// One choice inside a non-streaming [`ChatCompletionResponse`].
///
/// (No `Eq`: `logprobs` holds a `serde_json::Value`, which is not `Eq`.)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
    /// OpenAI `logprobs`; always `null` on this endpoint (logprobs are not produced), serialized
    /// rather than skipped so the choice shape matches OpenAI.
    pub logprobs: Option<serde_json::Value>,
}

/// Assemble a single-choice `chat.completion` response from a complete answer (spec D2).
///
/// `created` is supplied by the caller (never taken from an ambient clock in this pure layer);
/// `usage` is the caller's accumulated total — zero when the buffered pipeline reports none
/// (spec D3: the buffered `OpenAiLlm` emits no `Usage`).
pub fn build_response(
    answer: &str,
    id: &str,
    model: &str,
    created: i64,
    usage: Usage,
) -> ChatCompletionResponse {
    ChatCompletionResponse {
        id: id.to_string(),
        object: "chat.completion",
        created,
        model: model.to_string(),
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".into(),
                content: answer.to_string(),
            },
            finish_reason: "stop".into(),
            logprobs: None,
        }],
        usage,
        system_fingerprint: None,
    }
}

// ──── OpenAI error envelope (spec Errors) ────

/// OpenAI error `type` for a malformed request (HTTP 400).
pub const ERR_INVALID_REQUEST: &str = "invalid_request_error";
/// OpenAI error `type` for an upstream capability failure (HTTP 502).
pub const ERR_UPSTREAM: &str = "upstream_error";
/// OpenAI error `type` for an internal / unavailable condition (HTTP 5xx).
pub const ERR_SERVER: &str = "server_error";

/// OpenAI error envelope: `{"error": {"message": ..., "type": ...}}`.
///
/// Every non-2xx response from `/v1/chat/completions` uses this shape (not the host's flat
/// `{"error": "..."}`) so an OpenAI client / agentgateway parses it as a standard error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenAiErrorBody {
    pub error: OpenAiError,
}

/// The inner object of an [`OpenAiErrorBody`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OpenAiError {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
}

impl OpenAiErrorBody {
    /// Build an error envelope from an OpenAI `type` and a human message.
    pub fn new(error_type: &str, message: impl Into<String>) -> Self {
        Self {
            error: OpenAiError {
                message: message.into(),
                error_type: error_type.to_string(),
            },
        }
    }
}

impl MapError {
    /// Map a mapping failure onto `(http_status, openai_error_type, message)`.
    ///
    /// Every mapping failure is a client error (ERR2 → HTTP 400 `invalid_request_error`).
    pub fn to_openai(&self) -> (u16, &'static str, String) {
        match self {
            MapError::NoUserMessage => (
                400,
                ERR_INVALID_REQUEST,
                "`messages` must contain at least one `user` message".to_string(),
            ),
            MapError::BadShape(reason) => (400, ERR_INVALID_REQUEST, reason.clone()),
        }
    }
}

/// The OpenAI error `type` for a runtime-prelude / pipeline HTTP status (spec Errors table):
/// any 4xx is a client `invalid_request_error`, `502` is an `upstream_error`, and every other
/// 5xx is a `server_error`.
pub fn error_type_for_status(status: u16) -> &'static str {
    match status {
        400..=499 => ERR_INVALID_REQUEST,
        502 => ERR_UPSTREAM,
        _ => ERR_SERVER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str) -> ChatMessage {
        ChatMessage {
            role: role.into(),
            content: content.into(),
        }
    }

    #[test]
    fn maps_single_user_message_to_prompt() {
        let req = map_request(vec![msg("user", "Q")]).unwrap();
        assert_eq!(req.prompt, "Q");
        assert!(req.history.is_empty());
        assert!(req.session_id.is_none());
    }

    #[test]
    fn pairs_earlier_turns_into_history() {
        let req = map_request(vec![msg("user", "A"), msg("assistant", "a"), msg("user", "B")]).unwrap();
        assert_eq!(req.prompt, "B");
        assert_eq!(
            req.history,
            vec![History {
                user_prompt: "A".into(),
                model_response: "a".into(),
            }]
        );
    }

    #[test]
    fn ignores_system_messages() {
        let req = map_request(vec![msg("system", "sys"), msg("user", "Q")]).unwrap();
        assert_eq!(req.prompt, "Q");
        assert!(req.history.is_empty());
    }

    #[test]
    fn ignores_unknown_roles_like_tool() {
        // Non-conversational roles (tool / function / anything not user|assistant) are dropped,
        // not folded into history — replaying a transcript that carries tool messages must not
        // pollute the user turns.
        let req = map_request(vec![
            msg("user", "A"),
            msg("assistant", "a"),
            msg("tool", "toolresult"),
            msg("user", "B"),
        ])
        .unwrap();
        assert_eq!(req.prompt, "B");
        assert_eq!(
            req.history,
            vec![History {
                user_prompt: "A".into(),
                model_response: "a".into(),
            }]
        );
    }

    #[test]
    fn empty_messages_is_error() {
        assert_eq!(map_request(vec![]).unwrap_err(), MapError::NoUserMessage);
    }

    #[test]
    fn only_system_is_error() {
        assert_eq!(
            map_request(vec![msg("system", "s")]).unwrap_err(),
            MapError::NoUserMessage
        );
    }

    #[test]
    fn merges_adjacent_same_role_user_turns_into_one_prompt() {
        // Relaxed mapping (was a 400): two adjacent `user` turns are no longer rejected — they
        // fold into a single prompt, their content joined with `\n`.
        let req = map_request(vec![msg("user", "A"), msg("user", "B")]).unwrap();
        assert_eq!(req.prompt, "A\nB");
        assert!(req.history.is_empty());
    }

    #[test]
    fn merges_adjacent_same_role_in_history() {
        // `[user:A1, user:A2, assistant:b, user:C]`: the two leading users merge into one history
        // turn (`A1\nA2`), paired with `b`; `C` is the current prompt.
        let req = map_request(vec![
            msg("user", "A1"),
            msg("user", "A2"),
            msg("assistant", "b"),
            msg("user", "C"),
        ])
        .unwrap();
        assert_eq!(req.prompt, "C");
        assert_eq!(
            req.history,
            vec![History {
                user_prompt: "A1\nA2".into(),
                model_response: "b".into(),
            }]
        );
    }

    #[test]
    fn leading_assistant_pairs_with_an_empty_user_prompt() {
        // A conversation that opens with an assistant turn: pair it with an empty `user_prompt`
        // rather than rejecting the shape.
        let req = map_request(vec![msg("assistant", "a"), msg("user", "B")]).unwrap();
        assert_eq!(req.prompt, "B");
        assert_eq!(
            req.history,
            vec![History {
                user_prompt: String::new(),
                model_response: "a".into(),
            }]
        );
    }

    #[test]
    fn developer_role_is_ignored_like_system() {
        // OpenAI's `developer` role is the new system synonym — dropped, same as `system`.
        let req = map_request(vec![msg("developer", "be brief"), msg("user", "Q")]).unwrap();
        assert_eq!(req.prompt, "Q");
        assert!(req.history.is_empty());
    }

    #[test]
    fn content_parts_array_concatenates_text_parts() {
        // OpenAI clients may send `content` as an array of typed parts. Text parts are
        // concatenated; non-text parts (e.g. `image_url`) are ignored.
        let m: ChatMessage = serde_json::from_value(serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "Hello "},
                {"type": "image_url", "image_url": {"url": "http://x/y.png"}},
                {"type": "text", "text": "world"}
            ]
        }))
        .expect("content-parts array should deserialize");
        assert_eq!(m.role, "user");
        assert_eq!(m.content, "Hello world");
    }

    #[test]
    fn content_parts_array_maps_through_to_prompt() {
        // End to end: a parts-array user message becomes the prompt with its text concatenated.
        let req: ChatCompletionRequest = serde_json::from_str(
            r#"{"model":"m","messages":[{"role":"user","content":[{"type":"text","text":"上個月"},{"type":"text","text":"營收?"}]}]}"#,
        )
        .unwrap();
        let mapped = map_request(req.messages).unwrap();
        assert_eq!(mapped.prompt, "上個月營收?");
    }

    #[test]
    fn accumulates_usage_across_stages() {
        let usages = vec![
            UsageData { prompt: 10, completion: 5, reasoning: None, total: 15 },
            UsageData { prompt: 8, completion: 4, reasoning: Some(3), total: 12 },
        ];
        let u = accumulate_usage(&usages);
        assert_eq!(u.prompt_tokens, 18);
        assert_eq!(u.completion_tokens, 9);
        assert_eq!(u.total_tokens, 27);
        assert_eq!(
            u.completion_tokens_details,
            Some(ReasoningDetails { reasoning_tokens: 3 })
        );
    }

    #[test]
    fn accumulates_usage_without_reasoning() {
        let u = accumulate_usage(&[UsageData { prompt: 1, completion: 2, reasoning: None, total: 3 }]);
        assert!(u.completion_tokens_details.is_none());
    }

    #[test]
    fn build_chunks_streams_answer_with_role_and_stop() {
        let chunks = build_chunks("hello world", "chatcmpl-x", "m", 0, false);
        // object tag correct
        assert_eq!(chunks[0].object, "chat.completion.chunk");
        // leading chunk carries role
        assert_eq!(chunks.first().unwrap().choices[0].delta.role.as_deref(), Some("assistant"));
        // content chunks concatenate back to the original answer
        let content: String = chunks
            .iter()
            .filter_map(|c| c.choices[0].delta.content.clone())
            .collect();
        assert_eq!(content, "hello world");
        // terminal chunk: empty delta + finish_reason "stop"
        let last = chunks.last().unwrap();
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));
        assert!(last.choices[0].delta.role.is_none());
        assert!(last.choices[0].delta.content.is_none());
    }

    #[test]
    fn build_chunks_handles_empty_answer() {
        // TC-B05: empty answer still yields a valid terminating sequence.
        let chunks = build_chunks("", "id", "m", 0, false);
        let content: String = chunks
            .iter()
            .filter_map(|c| c.choices[0].delta.content.clone())
            .collect();
        assert_eq!(content, "");
        assert_eq!(chunks.last().unwrap().choices[0].finish_reason.as_deref(), Some("stop"));
    }

    // ── non-streaming response (spec D2 / AC-1) ──

    #[test]
    fn build_response_wraps_answer_as_a_single_stop_choice() {
        let usage = Usage {
            prompt_tokens: 3,
            completion_tokens: 4,
            total_tokens: 7,
            completion_tokens_details: None,
        };
        let resp = build_response("the answer", "chatcmpl-1", "rd-model", 1_700, usage.clone());
        assert_eq!(resp.object, "chat.completion");
        assert_eq!(resp.id, "chatcmpl-1");
        assert_eq!(resp.created, 1_700);
        assert_eq!(resp.model, "rd-model");
        assert_eq!(resp.choices.len(), 1);
        let choice = &resp.choices[0];
        assert_eq!(choice.index, 0);
        assert_eq!(choice.finish_reason, "stop");
        assert_eq!(choice.message.role, "assistant");
        assert_eq!(choice.message.content, "the answer");
        assert_eq!(resp.usage, usage);
    }

    #[test]
    fn response_serializes_to_the_openai_wire_shape() {
        let resp = build_response("hi", "chatcmpl-x", "m", 42, Usage::default());
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["id"], "chatcmpl-x");
        assert_eq!(v["choices"][0]["index"], 0);
        assert_eq!(v["choices"][0]["message"]["role"], "assistant");
        assert_eq!(v["choices"][0]["message"]["content"], "hi");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        // usage present with zeros when the buffered pipeline reports none (D3).
        assert_eq!(v["usage"]["prompt_tokens"], 0);
        assert_eq!(v["usage"]["total_tokens"], 0);
    }

    // ── OpenAI error envelope + status mapping (spec Errors) ──

    #[test]
    fn map_error_maps_to_400_invalid_request() {
        // ERR2: no user message.
        let (status, etype, _msg) = MapError::NoUserMessage.to_openai();
        assert_eq!(status, 400);
        assert_eq!(etype, ERR_INVALID_REQUEST);
        // ERR2: bad conversation shape carries its own diagnostic message through.
        let (status, etype, msg) = MapError::BadShape("adjacent user turns".into()).to_openai();
        assert_eq!(status, 400);
        assert_eq!(etype, ERR_INVALID_REQUEST);
        assert_eq!(msg, "adjacent user turns");
    }

    #[test]
    fn error_type_tracks_the_http_status() {
        assert_eq!(error_type_for_status(400), ERR_INVALID_REQUEST); // ERR3 too-long, ERR2
        assert_eq!(error_type_for_status(404), ERR_INVALID_REQUEST);
        assert_eq!(error_type_for_status(502), ERR_UPSTREAM); // ERR5 capability
        assert_eq!(error_type_for_status(503), ERR_SERVER); // ERR1 runtime off
    }

    #[test]
    fn openai_error_body_serializes_to_the_nested_envelope() {
        let body = OpenAiErrorBody::new(ERR_INVALID_REQUEST, "bad");
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            serde_json::json!({"error": {"message": "bad", "type": "invalid_request_error"}})
        );
    }

    // ── streaming usage: OpenAI `stream_options.include_usage` (D3) ──

    #[test]
    fn stream_options_deserializes_include_usage() {
        let opts: StreamOptions = serde_json::from_str(r#"{"include_usage":true}"#).unwrap();
        assert!(opts.include_usage);
        // Absent flag defaults to false (serde default).
        let opts: StreamOptions = serde_json::from_str("{}").unwrap();
        assert!(!opts.include_usage);
    }

    #[test]
    fn request_stream_options_default_none_and_parse() {
        // Absent → None (opting out of the usage-only chunk, wire unchanged).
        let req: ChatCompletionRequest =
            serde_json::from_str(r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#)
                .unwrap();
        assert!(req.stream_options.is_none());
        // Present → carried through.
        let req: ChatCompletionRequest = serde_json::from_str(
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true,"stream_options":{"include_usage":true}}"#,
        )
        .unwrap();
        assert!(req.stream_options.unwrap().include_usage);
    }

    #[test]
    fn content_chunks_omit_the_usage_field() {
        // The ordinary streamed chunks never carry `usage` (serde skips `None`), so the existing
        // wire shape is unchanged when `include_usage` is off.
        for chunk in build_chunks("hello world", "id", "m", 0, false) {
            let v = serde_json::to_value(&chunk).unwrap();
            assert!(
                v.get("usage").is_none(),
                "content chunk must not serialize a usage field"
            );
        }
    }

    #[test]
    fn content_chunks_carry_null_usage_when_include_usage() {
        // OpenAI `include_usage` contract: once the client opts in, EVERY ordinary chunk carries an
        // explicit `usage: null` (present, not omitted); only the terminal usage-only chunk from
        // `usage_chunk` carries the real total. (Finding #5.)
        for chunk in build_chunks("hello world", "id", "m", 0, true) {
            let v = serde_json::to_value(&chunk).unwrap();
            assert!(
                v.as_object().unwrap().contains_key("usage"),
                "content chunk must carry a usage key when include_usage is on: {v}"
            );
            assert!(
                v["usage"].is_null(),
                "content chunk usage must be null when include_usage is on, got {}",
                v["usage"]
            );
        }
    }

    #[test]
    fn choices_carry_logprobs_null_for_openai_compat() {
        // OpenAI puts a `logprobs` field on every choice (null unless logprobs were requested).
        let resp = build_response("hi", "id", "m", 0, Usage::default());
        let v = serde_json::to_value(&resp).unwrap();
        let choice = v["choices"][0].as_object().unwrap();
        assert!(
            choice.contains_key("logprobs"),
            "non-stream choice must carry a logprobs field"
        );
        assert!(choice["logprobs"].is_null());

        // Streaming chunk choices carry it too (the leading role chunk has a choice).
        let chunks = build_chunks("hello", "id", "m", 0, false);
        let cv = serde_json::to_value(&chunks[0]).unwrap();
        let cchoice = cv["choices"][0].as_object().unwrap();
        assert!(
            cchoice.contains_key("logprobs"),
            "chunk choice must carry a logprobs field"
        );
        assert!(cchoice["logprobs"].is_null());
    }

    #[test]
    fn response_and_chunk_carry_system_fingerprint_field() {
        // OpenAI includes `system_fingerprint` on the response/chunk envelope; we compute none, so
        // it serializes as null (present, not omitted) to stay close to the OpenAI wire.
        let resp = build_response("hi", "id", "m", 0, Usage::default());
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v.as_object().unwrap().contains_key("system_fingerprint"));
        assert!(v["system_fingerprint"].is_null());

        let chunk = &build_chunks("hi", "id", "m", 0, false)[0];
        let cv = serde_json::to_value(chunk).unwrap();
        assert!(cv.as_object().unwrap().contains_key("system_fingerprint"));
        assert!(cv["system_fingerprint"].is_null());
    }

    #[test]
    fn usage_chunk_serializes_with_empty_choices_and_usage() {
        // TC: usage-only chunk pins to `{"choices":[],"usage":{...},...}` (OpenAI include_usage).
        let usage = Usage {
            prompt_tokens: 12,
            completion_tokens: 8,
            total_tokens: 20,
            completion_tokens_details: None,
        };
        let chunk = usage_chunk(usage, "chatcmpl-x", "m", 42);
        let v = serde_json::to_value(&chunk).unwrap();
        assert_eq!(v["object"], "chat.completion.chunk");
        assert_eq!(v["id"], "chatcmpl-x");
        assert_eq!(v["created"], 42);
        assert_eq!(v["model"], "m");
        assert!(v["choices"].as_array().unwrap().is_empty());
        assert_eq!(v["usage"]["prompt_tokens"], 12);
        assert_eq!(v["usage"]["completion_tokens"], 8);
        assert_eq!(v["usage"]["total_tokens"], 20);
    }
}
