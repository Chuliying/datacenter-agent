//! Token-by-token streaming against OpenRouter.

use tracing::{debug, error, info};

use futures::{Stream, StreamExt};

use super::client::build_client;
use super::system_prompt::FIXED_SYSTEM_PROMPT;
use crate::model::GenerationConfig;

/// An OpenRouter SSE streaming frame result.
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// A token fragment to append to the answer.
    Token(String),
    /// The model finished cleanly.
    Done,
    /// Terminated with error.
    Error(String),
}

/// A detector to sense stream termination.
///
/// The *actual* cancellation is automatic and needs no code, while axum dropped
/// the SSE response body, this generator future is dropped, which drops the
/// `stream` local below with underlying reqwest OpenRouter connection.
///
/// We add this stuff here to increase observability.
struct TerminationDetector {
    completed: bool,
}

impl Drop for TerminationDetector {
    fn drop(&mut self) {
        if !self.completed {
            info!("client disconnected mid-stream, upstream request aborted");
        }
    }
}

/// Stream tokens using [`FIXED_SYSTEM_PROMPT`] — the connector-owned
/// analytical prompt. Mirrors [`super::generate`].
pub fn token_stream(cfg: GenerationConfig) -> impl Stream<Item = LlmEvent> {
    token_stream_with_system(cfg, FIXED_SYSTEM_PROMPT.to_string())
}

/// Stream tokens with a caller-supplied system prompt that fully replaces
/// [`FIXED_SYSTEM_PROMPT`]. Mirrors [`super::generate_with_system`].
pub fn token_stream_with_system(
    cfg: GenerationConfig,
    system: String,
) -> impl Stream<Item = LlmEvent> {
    async_stream::stream! {

        // Build the client.
        let client = match build_client(&cfg) {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "failed to build OpenRouter client");
                yield LlmEvent::Error(format!("{e:#}"));
                return;
            }
        };

        // Cast `GenerationConfig` to OpenRouter chat request.
        let req = match cfg.to_chat_request(&system) {
            Ok(r) => r,
            Err(e) => {
                error!(error = %e, "failed to build chat request");
                yield LlmEvent::Error(format!("{e:#}"));
                return;
            }
        };

        debug!(messages = ?req.messages, "llm.generate.request_built");

        // Open the LLM stream. `create_stream` sets `stream: true` for us.
        let mut stream = match client.chat().create_stream(req).await {
            Ok(s) => s,
            Err(e) => {
                error!(error = %e, "openrouter create_stream failed");
                yield LlmEvent::Error(e.to_string());
                return;
            }
        };

        debug!("streaming started");

        // Tracks the stream status if been dropped.
        let mut guard = TerminationDetector { completed: false };

        // Process frames
        while let Some(frame) = stream.next().await {
            match frame {
                Ok(chunk) => {
                    if let Some(choice) = chunk.choices.first() {
                        if let Some(tok) = &choice.delta.content {
                            if !tok.is_empty() {
                                yield LlmEvent::Token(tok.clone());
                            }
                        }
                        // `finish_reason` set -> this choice is complete.
                        if choice.finish_reason.is_some() {
                            debug!(reason = ?choice.finish_reason, "streaming finished");
                            guard.completed = true;
                            yield LlmEvent::Done;
                            return;
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "error occurred during stream");
                    guard.completed = true; // an explicit error is still a clean end
                    yield LlmEvent::Error(e.to_string());
                    return;
                }
            }
        }

        // If we reach here without having seen a finish_reason,
        // we still treat the stream closed successfully, emit Done so the
        // client closes cleanly.
        guard.completed = true;
        yield LlmEvent::Done;
    }
}
