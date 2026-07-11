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

//! OpenRouter LLM connector with MCP tool-calling.
//!
//! The single entry point is the agentic loop, exposed as two functions:
//! - [`agent_stream`]: drive the tool-calling loop, streaming the final answer
//!   token-by-token (used by the legacy `/report/stream` path and the runtime turn).
//! - [`generate`]: run the same loop and await the whole Markdown reply (used
//!   by the legacy `/report` path and the greeting generator).

mod agent;
mod client;

pub use agent::{agent_stream, generate, LlmEvent};
