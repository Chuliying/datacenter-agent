//! OpenRouter LLM connector.
//!
//! Two entry points:
//! - [`generate`]: issue one chat completion and await the full
//!   Markdown reply.
//! - [`token_stream`]: return an `impl Stream<Item = LlmEvent>` that
//!   yields tokens as they arrive (used by `/agent/stream`).

mod client;
mod generate;
mod streamer;

pub use generate::generate;
pub use streamer::{token_stream, LlmEvent};
