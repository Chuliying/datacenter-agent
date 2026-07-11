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

//! The sub-agent layer: config-driven and code-driven agents composed into pipelines.
//!
//! This is the port of three sibling contracts into `datacenter-agent`, one module per concern.
//!
//! The submodules are:
//!
//! - [`payload`] — the [`AgentPayload`](payload::AgentPayload) sum type, the abstract
//!   [`Tool`](payload::Tool) / [`LlmCapability`](payload::LlmCapability) capabilities, the
//!   [`ToolOutcome`](payload::ToolOutcome) retry model, and the tool-use loop
//!   ([`run_llm_loop`](payload::run_llm_loop)). Port of the `agent_payload` contract.
//! - [`tools`] — the closed logical [`ToolId`](tools::ToolId), the backend-agnostic
//!   [`ToolRegistry`](tools::ToolRegistry) (boot-resolved, completeness-checked), the generic
//!   validating [`SchemaTool<T>`](tools::SchemaTool), and the MCP-backed
//!   [`McpTool`](tools::McpTool). Port of the `tool` contract.
//! - [`config`] — the authored [`SubAgentConfig`](config::SubAgentConfig) surface, the LLM
//!   provider model, and the boot resolution rules ([`resolve_llm`](config::resolve_llm),
//!   [`effective_output`](config::effective_output)). Port of the `sub_agent` contract, PART A.
//! - [`engine`] — the [`SubAgent`](engine::SubAgent) trait unifying config-defined
//!   ([`ConfiguredAgent`](engine::ConfiguredAgent)) and code-defined
//!   ([`HelloWorld`](engine::HelloWorld)) agents, plus the [`Orchestrator`](engine::Orchestrator).
//!   Port of the `sub_agent` contract, PART B.
//! - [`llm`] — the concrete [`OpenAiLlm`](llm::OpenAiLlm) buffered adapter and the
//!   [`StreamingOpenAiLlm`](llm::StreamingOpenAiLlm) token-streaming sibling that turn a
//!   [`ResolvedLlm`](config::ResolvedLlm) into an [`LlmCapability`](payload::LlmCapability),
//!   built against the async-openai already in the tree.
//! - [`events`] — the streaming event model: one injected [`EventSink`](events::EventSink)
//!   carrying one tagged [`AgentEvent`](events::AgentEvent), emitted by the LLM adapter, the
//!   tool wrapper, and the orchestrator (plan §8).
//!
//! Nothing here is wired into [`AppState`](crate::appstate::AppState) yet.
//! The modules are dormant, unit-tested groundwork.
//!
//! # References
//!
//! - Payload contract — `.spec/contract/agent_payload`
//! - Tool contract — `.spec/contract/tool`
//! - Sub-agent contract — `.spec/contract/sub_agent`

pub mod config;
pub mod engine;
pub mod events;
pub mod llm;
pub mod payload;
pub mod tools;
