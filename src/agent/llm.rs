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

use anyhow::{Context, Result};
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionToolChoiceOption,
    ChatCompletionTools, CreateChatCompletionRequestArgs, FunctionCall, FunctionObject,
    ToolChoiceOptions,
};
use async_openai::Client;
use async_trait::async_trait;

use crate::agent::config::ResolvedLlm;
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
        let http = reqwest::Client::builder()
            .build()
            .context("build OpenAiLlm http client")?;
        let cfg = OpenAIConfig::new()
            .with_api_base(&resolved.base_url)
            .with_api_key(resolved.api_key.clone().unwrap_or_default());
        Ok(Self {
            client: Client::with_config(cfg).with_http_client(http),
            model: resolved.model.clone(),
            temperature: resolved.temperature,
            top_p: resolved.top_p,
            max_tokens: resolved.max_tokens,
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
                id: f.id,
                // The model streams arguments as a JSON *string*; a blank string means
                // "no arguments". Parse leniently, defaulting to an empty object.
                arguments: if f.function.arguments.trim().is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&f.function.arguments)
                        .unwrap_or(serde_json::Value::String(f.function.arguments.clone()))
                },
                name: f.function.name,
            }),
            ChatCompletionMessageToolCalls::Custom(_) => None,
        })
        .collect()
}

#[async_trait]
impl LlmCapability for OpenAiLlm {
    async fn chat(
        &self,
        messages: &[LlmMessage],
        tools: &[ToolSchema],
    ) -> Result<LlmResponse, AgentError> {
        let request_messages = messages
            .iter()
            .map(to_request_message)
            .collect::<Result<Vec<_>, _>>()?;

        let mut builder = CreateChatCompletionRequestArgs::default();
        builder
            .model(&self.model)
            .messages(request_messages)
            .temperature(self.temperature)
            .top_p(self.top_p)
            .max_tokens(self.max_tokens);

        if !tools.is_empty() {
            builder.tools(to_request_tools(tools)).tool_choice(
                ChatCompletionToolChoiceOption::Mode(ToolChoiceOptions::Auto),
            );
        }

        let request = builder
            .build()
            .map_err(|e| AgentError::Capability(format!("build chat request: {e}")))?;

        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| AgentError::Capability(format!("chat completion: {e}")))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::Provider;

    fn resolved() -> ResolvedLlm {
        ResolvedLlm {
            provider: Provider::OpenRouter,
            base_url: "https://openrouter.ai/api/v1".into(),
            model: "google/gemini-flash".into(),
            temperature: 0.2,
            top_p: 0.1,
            max_tokens: 256,
            api_key: Some("sk-test".into()),
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
}
