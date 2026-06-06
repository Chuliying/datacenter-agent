//! OpenRouter LLM connector with MCP tool-calling.
//!
//! The single entry point is the agentic loop in [`agent`]:
//! - [`agent_stream`]: drive the tool-calling loop, streaming the final answer
//!   token-by-token (used by `/agent/stream`).
//! - [`generate`]: run the same loop and await the whole Markdown reply (used
//!   by `/agent` and the greeting generator).

mod agent;
mod client;

pub use agent::{agent_stream, generate, LlmEvent};
