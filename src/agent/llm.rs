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

//! The concrete [`LlmCapability`] adapter, built against the async-openai already in the tree.
//!
//! This is the `ResolvedLlm` → capability factory the sub-agent contract defers to the
//! implementation plan.
//!
//! It is the **buffered** (non-streaming) `chat` used by upstream stages such as the fetcher;
//! the streaming terminal-stage path is a later plan item.
//!
//! It targets async-openai **0.40** deliberately.
//! The contract's reference adapter pins 0.41.1, but the crate and the production loop are on
//! 0.40, so we avoid a crate-wide bump here.
//!
//! # References
//!
//! - Sub-agent plan §8 — buffered now, streaming terminal stage later; pin async-openai 0.40

#![allow(dead_code)] // groundwork: not yet constructed from a boot-built registry.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionStreamOptions,
    ChatCompletionStreamResponseDelta, ChatCompletionTool, ChatCompletionToolChoiceOption,
    ChatCompletionTools, CompletionUsage, CreateChatCompletionRequest,
    CreateChatCompletionRequestArgs, FinishReason, FunctionCall, FunctionObject, ReasoningEffort,
    ToolChoiceOptions,
};
use async_openai::Client;
use async_trait::async_trait;
use futures::StreamExt;
use tracing::info;

use crate::agent::config::{ReasoningEffort as ConfigReasoningEffort, ResolvedLlm};
use crate::agent::events::{AgentEvent, EventSink};
use crate::agent::payload::{
    AgentError, LlmCapability, LlmMessage, LlmResponse, ToolCall, ToolSchema,
};

/// An [`LlmCapability`] backed by an OpenAI-compatible endpoint (OpenRouter, Ollama, or any
/// `Custom`).
///
/// One instance per distinct [`ResolvedLlm`].
/// The model and sampling params are baked in, so `chat` only carries the per-turn messages and
/// tool schemas.
pub struct OpenAiLlm {
    client: Client<OpenAIConfig>,
    model: String,
    temperature: f32,
    top_p: f32,
    max_tokens: u32,
    reasoning_effort: Option<ReasoningEffort>,
}

/// Maps the config-side reasoning ladder onto the vendor's `reasoning_effort` enum.
fn to_reasoning_effort(effort: ConfigReasoningEffort) -> ReasoningEffort {
    match effort {
        ConfigReasoningEffort::Minimal => ReasoningEffort::Minimal,
        ConfigReasoningEffort::Low => ReasoningEffort::Low,
        ConfigReasoningEffort::Medium => ReasoningEffort::Medium,
        ConfigReasoningEffort::High => ReasoningEffort::High,
    }
}

impl OpenAiLlm {
    /// Builds a capability from a resolved LLM.
    ///
    /// Constructs the vendor client once (base URL plus bound key).
    /// A keyless provider (Ollama) passes an empty key.
    ///
    /// # Arguments
    ///
    /// - `resolved`: the fully-resolved LLM supplying base URL, key, model, and sampling params.
    ///
    /// # Returns
    ///
    /// Returns a ready [`OpenAiLlm`] on success.
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying HTTP client cannot be built.
    pub fn from_resolved(resolved: &ResolvedLlm) -> Result<Self> {
        Ok(Self {
            client: build_client(resolved)?,
            model: resolved.model.clone(),
            temperature: resolved.temperature,
            top_p: resolved.top_p,
            max_tokens: resolved.max_tokens,
            reasoning_effort: resolved.reasoning_effort.map(to_reasoning_effort),
        })
    }
}

/// Translates one abstract [`LlmMessage`] into an async-openai request message.
///
/// # Arguments
///
/// - `message`: the abstract message to translate.
///
/// # Returns
///
/// Returns the equivalent async-openai request message.
///
/// # Errors
///
/// - [`AgentError::Capability`] — async-openai's message builder rejected the content.
fn to_request_message(message: &LlmMessage) -> Result<ChatCompletionRequestMessage, AgentError> {
    let build_err = |e: async_openai::error::OpenAIError| {
        AgentError::Capability(format!("build chat message: {e}"))
    };
    match message {
        LlmMessage::System(content) => Ok(ChatCompletionRequestSystemMessageArgs::default()
            .content(content.clone())
            .build()
            .map_err(build_err)?
            .into()),
        LlmMessage::User(content) => Ok(ChatCompletionRequestUserMessageArgs::default()
            .content(content.clone())
            .build()
            .map_err(build_err)?
            .into()),
        LlmMessage::Assistant {
            content,
            tool_calls,
        } => {
            let mut assistant = ChatCompletionRequestAssistantMessageArgs::default();
            if let Some(text) = content {
                assistant.content(text.clone());
            }
            if !tool_calls.is_empty() {
                let calls: Vec<ChatCompletionMessageToolCalls> = tool_calls
                    .iter()
                    .map(|tc| {
                        ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                            id: tc.id.clone(),
                            function: FunctionCall {
                                name: tc.name.clone(),
                                arguments: tc.arguments.to_string(),
                            },
                        })
                    })
                    .collect();
                assistant.tool_calls(calls);
            }
            Ok(assistant.build().map_err(build_err)?.into())
        }
        LlmMessage::Tool {
            tool_call_id,
            content,
        } => Ok(ChatCompletionRequestToolMessageArgs::default()
            .content(content.clone())
            .tool_call_id(tool_call_id.clone())
            .build()
            .map_err(build_err)?
            .into()),
    }
}

/// Advertises the agent's granted tools to the model.
///
/// # Arguments
///
/// - `tools`: the granted tool schemas to advertise.
///
/// # Returns
///
/// Returns the async-openai tool list (empty when `tools` is empty).
fn to_request_tools(tools: &[ToolSchema]) -> Vec<ChatCompletionTools> {
    tools
        .iter()
        .map(|t| {
            ChatCompletionTools::Function(ChatCompletionTool {
                function: FunctionObject {
                    name: t.name.clone(),
                    description: Some(t.description.clone()),
                    parameters: Some(t.parameters.clone()),
                    strict: None,
                },
            })
        })
        .collect()
}

/// Maps a response message's tool calls (if any) into abstract [`ToolCall`]s.
///
/// Custom tool calls are ignored — this system only speaks function tools.
///
/// # Arguments
///
/// - `calls`: the tool calls carried on the response message.
///
/// # Returns
///
/// Returns the function tool calls as abstract [`ToolCall`]s; a blank argument string parses to
/// an empty object.
fn extract_tool_calls(calls: Vec<ChatCompletionMessageToolCalls>) -> Vec<ToolCall> {
    calls
        .into_iter()
        .filter_map(|call| match call {
            ChatCompletionMessageToolCalls::Function(f) => Some(ToolCall {
                arguments: parse_tool_arguments(&f.function.arguments),
                id: f.id,
                name: f.function.name,
            }),
            ChatCompletionMessageToolCalls::Custom(_) => None,
        })
        .collect()
}

// ===========================================================================
// Shared request / transport helpers (the buffered and streaming adapters both use these,
// so they send an identical request and differ only in `create` vs `create_stream`)
// ===========================================================================

/// Builds the vendor client for a resolved LLM (base URL plus bound key).
///
/// A keyless provider (Ollama) passes an empty key.
///
/// # Errors
///
/// Returns `Err` if the underlying HTTP client cannot be built.
fn build_client(resolved: &ResolvedLlm) -> Result<Client<OpenAIConfig>> {
    let http = reqwest::Client::builder()
        .build()
        .context("build OpenAiLlm http client")?;
    let cfg = OpenAIConfig::new()
        .with_api_base(&resolved.base_url)
        .with_api_key(resolved.api_key.clone().unwrap_or_default());
    Ok(Client::with_config(cfg).with_http_client(http))
}

/// Assembles one chat-completion request from abstract messages and tool schemas.
///
/// # Errors
///
/// - [`AgentError::Capability`] — a message failed to translate, or the request builder rejected
///   the assembled request.
fn build_request(
    model: &str,
    temperature: f32,
    top_p: f32,
    max_tokens: u32,
    reasoning_effort: Option<ReasoningEffort>,
    messages: &[LlmMessage],
    tools: &[ToolSchema],
) -> Result<CreateChatCompletionRequest, AgentError> {
    let request_messages = messages
        .iter()
        .map(to_request_message)
        .collect::<Result<Vec<_>, _>>()?;

    let mut builder = CreateChatCompletionRequestArgs::default();
    builder
        .model(model)
        .messages(request_messages)
        .temperature(temperature)
        .top_p(top_p)
        .max_tokens(max_tokens);

    // Lower (or raise) the reasoning budget when the stage asks for it; otherwise leave the
    // provider default by sending no `reasoning_effort` at all.
    if let Some(effort) = reasoning_effort {
        builder.reasoning_effort(effort);
    }

    if !tools.is_empty() {
        builder.tools(to_request_tools(tools)).tool_choice(
            ChatCompletionToolChoiceOption::Mode(ToolChoiceOptions::Auto),
        );
    }

    builder
        .build()
        .map_err(|e| AgentError::Capability(format!("build chat request: {e}")))
}

/// Parses a tool-call argument string, leniently.
///
/// The model streams arguments as a JSON *string*; a blank string means "no arguments" (some
/// providers use it for a zero-parameter call). Parse to a value, defaulting a blank to `{}` and a
/// non-JSON string to a `String` value rather than failing. Shared so the buffered and streaming
/// adapters key artifacts identically.
fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    if arguments.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(arguments)
            .unwrap_or_else(|_| serde_json::Value::String(arguments.to_string()))
    }
}

/// The reasoning-token count inside a usage report, when the provider breaks it out.
///
/// Reasoning tokens are billed and counted in `completion_tokens` but never streamed as content —
/// so this is the "hidden" budget behind a silent truncation.
fn reasoning_tokens(usage: &CompletionUsage) -> Option<u32> {
    usage
        .completion_tokens_details
        .as_ref()
        .and_then(|d| d.reasoning_tokens)
}

/// Lifts a provider [`CompletionUsage`] into the network-free [`AgentEvent::Usage`] (pure, so the
/// mapping — including the reasoning-token breakdown — is unit-testable without a transport).
fn usage_event(usage: &CompletionUsage) -> AgentEvent {
    AgentEvent::Usage {
        prompt: usage.prompt_tokens,
        completion: usage.completion_tokens,
        reasoning: reasoning_tokens(usage),
        total: usage.total_tokens,
    }
}

/// Logs one turn's token usage at INFO — the per-turn accounting both adapters share, so token
/// spend is visible in the server logs for both the buffered and streaming paths.
fn log_usage(usage: &CompletionUsage) {
    info!(
        prompt = usage.prompt_tokens,
        completion = usage.completion_tokens,
        reasoning = reasoning_tokens(usage),
        total = usage.total_tokens,
        "llm.usage"
    );
}

#[async_trait]
impl LlmCapability for OpenAiLlm {
    async fn chat(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse, AgentError> {
        let request = build_request(
            &self.model,
            self.temperature,
            self.top_p,
            self.max_tokens,
            self.reasoning_effort.clone(),
            messages,
            tools,
        )?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| AgentError::Capability(format!("chat completion: {e}")))?;

        // Token accounting for this turn (the buffered path has no sink, so this is log-only).
        if let Some(usage) = &response.usage {
            log_usage(usage);
        }

        let message = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| AgentError::Capability("chat response had no choices".into()))?
            .message;

        // Tool calls take precedence over any content: this turn is a tool round-trip.
        let calls = message
            .tool_calls
            .map(extract_tool_calls)
            .unwrap_or_default();
        if calls.is_empty() {
            Ok(LlmResponse::Message(message.content.unwrap_or_default()))
        } else {
            Ok(LlmResponse::ToolCalls(calls))
        }
    }
}

// ===========================================================================
// StreamingOpenAiLlm — the token-streaming sibling that emits AgentEvents (plan §8.3)
// ===========================================================================

/// One streamed delta, lifted out of async-openai's chunk into a network-free shape.
///
/// The extraction ([`extract_frame`]) is the only imperative-shell step; the fold over these
/// frames ([`StreamAccumulator`]) is pure and unit-testable without a transport.
struct DeltaFrame {
    content: Option<String>,
    tool_calls: Vec<ToolCallFragment>,
}

/// One streamed tool-call fragment: `id` / `name` arrive once, `arguments` dribble across frames.
struct ToolCallFragment {
    index: u32,
    id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

/// Lifts an async-openai stream delta into a [`DeltaFrame`] (the imperative shell).
fn extract_frame(delta: ChatCompletionStreamResponseDelta) -> DeltaFrame {
    let tool_calls = delta
        .tool_calls
        .unwrap_or_default()
        .into_iter()
        .map(|tc| ToolCallFragment {
            index: tc.index,
            id: tc.id,
            name: tc.function.as_ref().and_then(|f| f.name.clone()),
            arguments: tc.function.and_then(|f| f.arguments),
        })
        .collect();
    DeltaFrame {
        content: delta.content,
        tool_calls,
    }
}

/// One in-flight tool call, re-assembled by `index` from streamed fragments.
#[derive(Default)]
struct ToolFragment {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

/// The pure per-turn fold: accumulates content and tool-call fragments while emitting
/// [`AgentEvent`]s, then finalizes into one [`LlmResponse`] — the *same* value the buffered
/// `chat` returns, so [`run_llm_loop`](crate::agent::payload::run_llm_loop) is unchanged.
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    tools: BTreeMap<u32, ToolFragment>,
}

impl StreamAccumulator {
    /// Folds one frame, emitting an [`AgentEvent::ContentDelta`] per non-empty content chunk and
    /// an [`AgentEvent::ToolArgsDelta`] per non-empty argument fragment.
    fn push(&mut self, frame: DeltaFrame, sink: &dyn EventSink) {
        if let Some(text) = frame.content {
            if !text.is_empty() {
                self.content.push_str(&text);
                sink.emit(AgentEvent::ContentDelta { text });
            }
        }
        for frag in frame.tool_calls {
            let slot = self.tools.entry(frag.index).or_default();
            if let Some(id) = frag.id {
                slot.id = Some(id);
            }
            if let Some(name) = frag.name {
                slot.name = Some(name);
            }
            if let Some(args) = frag.arguments {
                if !args.is_empty() {
                    sink.emit(AgentEvent::ToolArgsDelta {
                        id: slot.id.clone().unwrap_or_default(),
                        fragment: args.clone(),
                    });
                    slot.arguments.push_str(&args);
                }
            }
        }
    }

    /// Finalizes the fold. With tool calls, emits an [`AgentEvent::ToolCallProposed`] per assembled
    /// call and returns [`LlmResponse::ToolCalls`]; otherwise returns the accumulated
    /// [`LlmResponse::Message`]. A fragment missing `id` or `name` is dropped — it cannot be
    /// dispatched.
    fn finish(self, sink: &dyn EventSink) -> LlmResponse {
        let calls: Vec<ToolCall> = self
            .tools
            .into_values()
            .filter_map(|slot| {
                Some(ToolCall {
                    arguments: parse_tool_arguments(&slot.arguments),
                    id: slot.id?,
                    name: slot.name?,
                })
            })
            .collect();
        if calls.is_empty() {
            LlmResponse::Message(self.content)
        } else {
            for call in &calls {
                sink.emit(AgentEvent::ToolCallProposed {
                    id: call.id.clone(),
                    name: call.name.clone(),
                });
            }
            LlmResponse::ToolCalls(calls)
        }
    }
}

/// The token-streaming [`LlmCapability`]: the *same* request as [`OpenAiLlm`], but `create_stream`
/// plus live [`AgentEvent`] emission to an injected [`EventSink`].
///
/// **Path A**: the sink is baked into the capability, so [`SubAgent`](crate::agent::engine::SubAgent)
/// and [`run_llm_loop`](crate::agent::payload::run_llm_loop) never see it and their signatures are
/// unchanged. `chat` still returns one complete [`LlmResponse`], so it drops into the existing
/// tool-use loop as-is; the streaming is a side effect on the sink.
pub struct StreamingOpenAiLlm {
    client: Client<OpenAIConfig>,
    model: String,
    temperature: f32,
    top_p: f32,
    max_tokens: u32,
    reasoning_effort: Option<ReasoningEffort>,
    sink: Arc<dyn EventSink>,
}

impl StreamingOpenAiLlm {
    /// Builds a streaming capability from a resolved LLM and the per-turn sink (Path A wiring).
    ///
    /// # Errors
    ///
    /// Returns `Err` if the underlying HTTP client cannot be built.
    pub fn from_resolved(resolved: &ResolvedLlm, sink: Arc<dyn EventSink>) -> Result<Self> {
        Ok(Self {
            client: build_client(resolved)?,
            model: resolved.model.clone(),
            temperature: resolved.temperature,
            top_p: resolved.top_p,
            max_tokens: resolved.max_tokens,
            reasoning_effort: resolved.reasoning_effort.map(to_reasoning_effort),
            sink,
        })
    }
}

#[async_trait]
impl LlmCapability for StreamingOpenAiLlm {
    async fn chat(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse, AgentError> {
        let mut request = build_request(
            &self.model,
            self.temperature,
            self.top_p,
            self.max_tokens,
            self.reasoning_effort.clone(),
            messages,
            tools,
        )?;
        // Ask the provider for a final usage chunk (sent before `[DONE]`, even when the turn stops
        // at the token limit), so token spend — including the hidden reasoning tokens behind a
        // silent burn — is always surfaced.
        request.stream_options = Some(ChatCompletionStreamOptions {
            include_usage: Some(true),
            include_obfuscation: None,
        });

        let mut stream = self
            .client
            .chat()
            .create_stream(request)
            .await
            .map_err(|e| AgentError::Capability(format!("open chat stream: {e}")))?;

        let sink = &*self.sink;
        let mut acc = StreamAccumulator::default();
        let mut finish_reason: Option<FinishReason> = None;
        let mut usage: Option<CompletionUsage> = None;

        while let Some(frame) = stream.next().await {
            let chunk =
                frame.map_err(|e| AgentError::Capability(format!("chat stream frame: {e}")))?;
            // The usage chunk arrives last, with an empty `choices` array — capture it before the
            // choice check (which would otherwise skip that chunk entirely).
            if let Some(u) = chunk.usage {
                usage = Some(u);
            }
            if let Some(choice) = chunk.choices.into_iter().next() {
                acc.push(extract_frame(choice.delta), sink);
                if let Some(reason) = choice.finish_reason {
                    // Record the stop reason but keep draining: the usage chunk follows this one,
                    // and the stream ends right after it.
                    finish_reason = Some(reason);
                }
            }
        }

        // Surface the turn's token usage *before* any truncation error, so a silently-burned budget
        // (e.g. reasoning tokens) is visible on the wire and in the logs even on the failure path.
        if let Some(u) = &usage {
            sink.emit(usage_event(u));
            log_usage(u);
        }

        // A truncated / filtered turn is a fatal capability error, not a silently-partial answer.
        match finish_reason {
            Some(FinishReason::Length) => {
                return Err(AgentError::Capability(
                    "chat response truncated at token limit".into(),
                ))
            }
            Some(FinishReason::ContentFilter) => {
                return Err(AgentError::Capability(
                    "chat response blocked by content filter".into(),
                ))
            }
            _ => {}
        }

        Ok(acc.finish(sink))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::Provider;
    use crate::agent::events::test_support::CollectingSink;

    fn resolved() -> ResolvedLlm {
        ResolvedLlm {
            provider: Provider::OpenRouter,
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "google/gemini-flash".into(),
            temperature: 0.2,
            top_p: 0.1,
            max_tokens: 256,
            api_key: Some("sk-test".into()),
            reasoning_effort: None,
        }
    }

    #[test]
    fn builds_a_client_from_a_resolved_llm() {
        // Construction is offline — no request is made — so this exercises the request-message
        // and tool translation wiring compiles and the client builds.
        let llm = OpenAiLlm::from_resolved(&resolved()).expect("client should build");
        assert_eq!(llm.model, "google/gemini-flash");
        assert_eq!(llm.max_tokens, 256);
    }

    #[test]
    fn translates_every_message_variant() {
        let messages = vec![
            LlmMessage::System("sys".into()),
            LlmMessage::User("hi".into()),
            LlmMessage::Assistant {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "bill_revenue".into(),
                    arguments: serde_json::json!({ "year": 2026 }),
                }],
            },
            LlmMessage::Tool {
                tool_call_id: "c1".into(),
                content: "rows".into(),
            },
        ];
        for m in &messages {
            to_request_message(m).expect("every message variant translates");
        }
        assert_eq!(to_request_tools(&[]).len(), 0);
    }

    #[test]
    fn build_request_serializes_reasoning_effort_only_when_set() {
        let msgs = [LlmMessage::User("hi".into())];
        // Set ⇒ the provider control reaches the wire, lowercased (`minimal`).
        let req =
            build_request("m", 0.2, 0.1, 256, Some(ReasoningEffort::Minimal), &msgs, &[]).unwrap();
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["reasoning_effort"], "minimal");

        // Unset ⇒ send nothing, so the provider default stands.
        let req_default = build_request("m", 0.2, 0.1, 256, None, &msgs, &[]).unwrap();
        let v_default = serde_json::to_value(&req_default).unwrap();
        assert!(
            v_default.get("reasoning_effort").is_none() || v_default["reasoning_effort"].is_null(),
            "reasoning_effort must be omitted when None, got {v_default:?}"
        );
    }

    #[test]
    fn extracts_function_tool_calls_and_parses_arguments() {
        let calls = vec![ChatCompletionMessageToolCalls::Function(
            ChatCompletionMessageToolCall {
                id: "c1".into(),
                function: FunctionCall {
                    name: "bill_revenue".into(),
                    arguments: "{\"year\":2026}".into(),
                },
            },
        )];
        let extracted = extract_tool_calls(calls);
        assert_eq!(extracted.len(), 1);
        assert_eq!(extracted[0].name, "bill_revenue");
        assert_eq!(extracted[0].arguments["year"], serde_json::json!(2026));
    }

    #[test]
    fn usage_event_surfaces_reasoning_tokens_when_present() {
        // A reasoning model reports the hidden reasoning-token subset of `completion_tokens`.
        let usage: CompletionUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 1200,
            "completion_tokens": 8000,
            "total_tokens": 9200,
            "completion_tokens_details": { "reasoning_tokens": 7600 }
        }))
        .unwrap();
        assert_eq!(
            usage_event(&usage),
            AgentEvent::Usage {
                prompt: 1200,
                completion: 8000,
                reasoning: Some(7600),
                total: 9200,
            }
        );
    }

    #[test]
    fn usage_event_reports_no_reasoning_for_a_plain_model() {
        // No `completion_tokens_details` ⇒ `reasoning` is `None`, not zero-guessed.
        let usage: CompletionUsage = serde_json::from_value(serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150
        }))
        .unwrap();
        assert_eq!(
            usage_event(&usage),
            AgentEvent::Usage {
                prompt: 100,
                completion: 50,
                reasoning: None,
                total: 150,
            }
        );
    }

    #[test]
    fn blank_tool_arguments_parse_as_empty_object() {
        let calls = vec![ChatCompletionMessageToolCalls::Function(
            ChatCompletionMessageToolCall {
                id: "c1".into(),
                function: FunctionCall {
                    name: "list_reports".into(),
                    arguments: "".into(),
                },
            },
        )];
        let extracted = extract_tool_calls(calls);
        assert_eq!(extracted[0].arguments, serde_json::json!({}));
    }

    // ── streaming accumulator: the pure fold from deltas to (events, LlmResponse), no network ──

    fn content_frame(text: &str) -> DeltaFrame {
        DeltaFrame {
            content: Some(text.to_string()),
            tool_calls: vec![],
        }
    }

    fn tool_frame(index: u32, id: Option<&str>, name: Option<&str>, args: Option<&str>) -> DeltaFrame {
        DeltaFrame {
            content: None,
            tool_calls: vec![ToolCallFragment {
                index,
                id: id.map(str::to_string),
                name: name.map(str::to_string),
                arguments: args.map(str::to_string),
            }],
        }
    }

    #[test]
    fn accumulator_streams_content_deltas_then_returns_the_joined_message() {
        let sink = CollectingSink::new();
        let mut acc = StreamAccumulator::default();
        acc.push(content_frame("Hel"), &sink);
        acc.push(content_frame("lo"), &sink);
        let response = acc.finish(&sink);

        assert!(matches!(response, LlmResponse::Message(m) if m == "Hello"));
        assert_eq!(
            sink.events(),
            vec![
                AgentEvent::ContentDelta { text: "Hel".into() },
                AgentEvent::ContentDelta { text: "lo".into() },
            ]
        );
    }

    #[test]
    fn accumulator_assembles_a_tool_call_from_argument_fragments() {
        let sink = CollectingSink::new();
        let mut acc = StreamAccumulator::default();
        // fragment 1: id + name + start of args; fragment 2: rest of args (same index, no id/name).
        acc.push(tool_frame(0, Some("call_1"), Some("bill_revenue"), Some("{\"year\":")), &sink);
        acc.push(tool_frame(0, None, None, Some("2026}")), &sink);
        let response = acc.finish(&sink);

        match response {
            LlmResponse::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_1");
                assert_eq!(calls[0].name, "bill_revenue");
                assert_eq!(calls[0].arguments, serde_json::json!({ "year": 2026 }));
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
        assert_eq!(
            sink.events(),
            vec![
                AgentEvent::ToolArgsDelta { id: "call_1".into(), fragment: "{\"year\":".into() },
                AgentEvent::ToolArgsDelta { id: "call_1".into(), fragment: "2026}".into() },
                AgentEvent::ToolCallProposed { id: "call_1".into(), name: "bill_revenue".into() },
            ]
        );
    }

    #[test]
    fn accumulator_treats_blank_arguments_as_an_empty_object() {
        let sink = CollectingSink::new();
        let mut acc = StreamAccumulator::default();
        acc.push(tool_frame(0, Some("c"), Some("list_reports"), None), &sink);
        match acc.finish(&sink) {
            LlmResponse::ToolCalls(calls) => {
                assert_eq!(calls[0].arguments, serde_json::json!({}));
            }
            other => panic!("expected ToolCalls, got {other:?}"),
        }
    }
}
