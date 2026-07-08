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

//! Shared root-level data model.

use anyhow::Result;
use async_openai::types::chat::{
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestUserMessageArgs,
};
use serde::{Deserialize, Serialize};

// ──── History ───

/// One previous turn of a conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    pub user_prompt: String,
    pub model_response: String,
}

// ──── GenerationConfig ───

/// Generation config for the LLM.
///
/// Everything the MCP tool-calling loop needs to seed and run one
/// conversation.
///
/// - `system` is the fully-resolved system prompt (base prompt + any MCP server
///   conventions).
/// - `history` is replayed as alternating user/assistant turns before the
///   current `user_prompt`, so the model can answer follow-ups from prior data
///   without re-fetching.
///
/// ## Warning
///
/// Do not serialize this struct because it contains sensitive information.
#[derive(Debug, Clone)]
pub struct GenerationConfig {
    /// System prompt to be sent to the LLM.
    pub system: String,
    /// User prompt to be sent to the LLM.
    pub user_prompt: String,
    /// History of the conversation.
    pub history: Vec<History>,
    /// API key to be sent to the LLM.
    pub api_key: String,
    /// Model to be used for generation.
    pub model: String,
    /// Base URL of OpenRouter API.
    pub base_url: String,
    /// Optional application repository URL for OpenRouter to identify
    /// project quota usage.
    pub app_url: Option<String>,
    /// Optional app title for OpenRouter.
    pub app_title: Option<String>,
    /// Optional temperature for LLM tuning.
    pub temperature: f32,
    /// Optional top-p for LLM tuning.
    pub top_p: f32,
    /// Optional max tokens for LLM tuning.
    pub max_tokens: u32,
}

impl GenerationConfig {
    /// Build the initial message list for the loop:
    /// `[system, h0.user, h0.assistant, ... , current_user]`.
    ///
    /// The loop appends assistant (with `tool_calls`) and `tool` messages onto
    /// this vector as it iterates.
    ///
    /// # Errors
    ///
    /// Returns `Err` if any message fails to build (effectively never for
    /// plain-text content).
    pub fn initial_messages(&self) -> Result<Vec<ChatCompletionRequestMessage>> {
        // Initialize messages with capacity for system prompt, history turns (2 per turn),
        // and current user prompt.
        let mut messages: Vec<ChatCompletionRequestMessage> =
            Vec::with_capacity(1 + self.history.len() * 2 + 1);

        // Push system prompt as chat message.
        messages.push(
            ChatCompletionRequestSystemMessageArgs::default()
                .content(self.system.clone())
                .build()?
                .into(),
        );

        let history_messages = self
            .history
            .iter()
            .map(|turn| {
                Ok([
                    ChatCompletionRequestUserMessageArgs::default()
                        .content(turn.user_prompt.clone())
                        .build()?
                        .into(),
                    ChatCompletionRequestAssistantMessageArgs::default()
                        .content(turn.model_response.clone())
                        .build()?
                        .into(),
                ])
            })
            .collect::<Result<Vec<[ChatCompletionRequestMessage; 2]>>>()?
            .into_iter()
            .flatten();

        messages.extend(history_messages);

        messages.push(
            ChatCompletionRequestUserMessageArgs::default()
                .content(self.user_prompt.clone())
                .build()?
                .into(),
        );

        Ok(messages)
    }
}
