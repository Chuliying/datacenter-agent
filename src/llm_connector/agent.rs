//! The MCP tool-calling loop — the heart of the agentic flow.
//!
//! Given a seeded conversation, drive a multi-turn exchange with the LLM. On
//! each turn the model may (a) ask to call one or more tools, or (b) produce a
//! final answer. When it asks for a tool we execute it against the `eomc-mcp`
//! server and feed the result back, then loop. This is the standard
//! OpenAI-compatible tool-calling loop:
//!
//! ```text
//!   seed ─▶ LLM (with tools) ─▶ tool_calls? ─yes─▶ MCP call_tool ─▶ tool result ─┐
//!                  ▲                                                              │
//!                  └──────────────────── append & loop ◀─────────────────────────┘
//!                                         │no
//!                                         ▼
//!                                 final Markdown answer (streamed)
//! ```
//!
//! Unlike the one-shot orchestrator, **every** turn is streamed: content
//! fragments are forwarded to the caller as [`LlmEvent::Token`] the instant
//! they arrive, while `tool_calls` fragments are buffered (re-assembled by
//! `index`) and executed silently between turns. So a chit-chat reply or the
//! final analysis streams live, and the intermediate tool round-trips are
//! invisible on the wire.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestToolMessageArgs, ChatCompletionTool, ChatCompletionToolChoiceOption,
    ChatCompletionTools, CreateChatCompletionRequest, CreateChatCompletionRequestArgs,
    FunctionCall, ToolChoiceOptions,
};
use futures::{Stream, StreamExt};
use tracing::{debug, error, info, warn};

use super::client::build_client;
use crate::mcp_client::McpHandle;
use crate::model::GenerationConfig;

/// Hard cap on tool-call rounds, so a misbehaving model can't loop forever.
const MAX_ITERATIONS: u32 = 16;

/// An OpenRouter streaming frame, surfaced to the HTTP layer.
#[derive(Debug, Clone)]
pub enum LlmEvent {
    /// A token fragment to append to the answer.
    Token(String),
    /// A `clear` event, just like `clear` in terminal.
    /// It indicated the previous tokens might be some intermediate thinking process,
    /// not the final answer. So the frontend can clear the answer buffer.
    Clear,
    /// The model produced its final answer and finished cleanly.
    Done,
    /// Terminated with an error (reported in-band, never as a `Result`).
    Error(String),
}

/// One in-flight tool call, re-assembled from streamed delta chunks.
///
/// The model sends `id`/`name` once and dribbles `arguments` across many chunks.
/// We need to **assemble** the tool call from the streamed chunks before executing it.
#[derive(Default)]
struct ToolSlot {
    /// The ID of the tool call, sent once at the beginning.
    id: Option<String>,
    /// The name of the tool call, sent once at the beginning.
    name: Option<String>,
    /// The arguments of the tool call, streamed in chunks.
    arguments: String,
}

/// A fully re-assembled tool call ready for execution.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedToolCall {
    /// The ID of the tool call, sent once at the beginning.
    id: String,
    /// The name of the tool call, sent once at the beginning.
    name: String,
    /// The arguments of the tool call, streamed in chunks.
    arguments: String,
}

/// Re-assemble the completed tool calls (drop any chunk that never
/// received an id + name since it can't be executed).
fn assemble_tool_calls(accum: BTreeMap<u32, ToolSlot>) -> Vec<ResolvedToolCall> {
    accum
        .into_values()
        .filter_map(|slot| {
            Some(ResolvedToolCall {
                id: slot.id?,
                name: slot.name?,
                arguments: slot.arguments,
            })
        })
        .collect()
}

/// Parsing the tool arguments
///
/// Tool arguments arrive as a JSON *string*, parse to an object.
fn parse_tool_arguments(args_str: &str) -> serde_json::Map<String, serde_json::Value> {
    serde_json::from_str(args_str).unwrap_or_else(|e| {
        warn!(error = %e, "could not parse tool arguments; using empty object");
        serde_json::Map::new()
    })
}

/// Build one chat-completion request assistant message from streamed turn content and tool calls.
fn build_assistant_message(
    content_buf: &str,
    tool_calls: &[ResolvedToolCall],
) -> Result<ChatCompletionRequestMessage> {
    let calls_enum: Vec<ChatCompletionMessageToolCalls> = tool_calls
        .iter()
        .map(|tc| {
            ChatCompletionMessageToolCalls::Function(ChatCompletionMessageToolCall {
                id: tc.id.clone(),
                function: FunctionCall {
                    name: tc.name.clone(),
                    arguments: tc.arguments.clone(),
                },
            })
        })
        .collect();

    let mut assistant = ChatCompletionRequestAssistantMessageArgs::default();
    if !content_buf.is_empty() {
        assistant.content(content_buf);
    }
    if !calls_enum.is_empty() {
        assistant.tool_calls(calls_enum);
    }
    let msg = assistant
        .build()
        .map_err(|e| anyhow!(e).context("building assistant message"))?;
    Ok(msg.into())
}

/// Build a tool message returning the output of a tool call.
fn build_tool_message(
    content: String,
    tool_call_id: String,
) -> Result<ChatCompletionRequestMessage> {
    let tool_msg = ChatCompletionRequestToolMessageArgs::default()
        .content(content)
        .tool_call_id(tool_call_id)
        .build()
        .map_err(|e| anyhow!(e).context("building tool message"))?;
    Ok(tool_msg.into())
}

/// Detects a stream dropped before completion.
///
/// We designed this to report the termination event. The cancellation itself is automatic.
struct TerminationDetector {
    /// Whether the stream completed successfully.
    completed: bool,
}

impl Drop for TerminationDetector {
    /// Report the termination event.
    ///
    /// (client disconnect → axum drops the response body → this future is dropped → the upstream request aborts)
    fn drop(&mut self) {
        if !self.completed {
            info!("client disconnected mid-stream, upstream request aborted");
        }
    }
}

/// Build one chat-completion request (streamed). Tools + `tool_choice: Auto`
/// are attached only when tools are present.
fn build_request(
    cfg: &GenerationConfig,
    messages: Vec<ChatCompletionRequestMessage>,
    tools: &[ChatCompletionTool],
) -> Result<CreateChatCompletionRequest> {
    let mut builder = CreateChatCompletionRequestArgs::default();
    builder
        .model(&cfg.model)
        .messages(messages)
        .temperature(cfg.temperature)
        .top_p(cfg.top_p)
        .max_tokens(cfg.max_tokens);

    if !tools.is_empty() {
        let defs: Vec<ChatCompletionTools> = tools
            .iter()
            .cloned()
            .map(ChatCompletionTools::Function)
            .collect();
        builder
            .tools(defs)
            // "auto" = let the model decide whether to call a tool.
            .tool_choice(ChatCompletionToolChoiceOption::Mode(
                ToolChoiceOptions::Auto,
            ));
    }

    builder
        .build()
        .map_err(|e| anyhow!(e).context("building chat completion request"))
}

/// Run the MCP tool-calling loop, streaming the final answer token-by-token.
///
/// The stream emits [`LlmEvent::Token`] fragments as the answer is produced,
/// then a terminal [`LlmEvent::Done`].
///
/// Failures (client/request build, stream open, mid-stream frame, or non-convergence)
/// are reported in-band as [`LlmEvent::Error`].
///
/// Dropping the stream cancels the upstream OpenRouter request.
pub fn agent_stream(
    cfg: GenerationConfig,
    tools: Arc<Vec<ChatCompletionTool>>,
    mcp: McpHandle,
) -> impl Stream<Item = LlmEvent> {
    async_stream::stream! {
        let client = match build_client(&cfg) {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "failed to build OpenRouter client");
                yield LlmEvent::Error(format!("{e:#}"));
                return;
            }
        };

        let mut messages = match cfg.initial_messages() {
            Ok(m) => m,
            Err(e) => {
                error!(error = %e, "failed to seed conversation");
                yield LlmEvent::Error(format!("{e:#}"));
                return;
            }
        };

        let mut guard = TerminationDetector { completed: false };
        let mut called_any_tool = false;

        for turn in 1..=MAX_ITERATIONS {
            info!(
                turn,
                model = %cfg.model,
                num_messages = messages.len(),
                num_tools = tools.len(),
                "→ OpenRouter chat request"
            );

            let req = match build_request(&cfg, messages.clone(), &tools) {
                Ok(r) => r,
                Err(e) => {
                    error!(error = %e, "failed to build chat request");
                    yield LlmEvent::Error(format!("{e:#}"));
                    return;
                }
            };

            let mut stream = match client.chat().create_stream(req).await {
                Ok(s) => s,
                Err(e) => {
                    error!(error = %e, "openrouter create_stream failed");
                    yield LlmEvent::Error(e.to_string());
                    return;
                }
            };

            // ──── Tool resolution & token accumulation ───


            // Per-turn accumulators: streamed answer text + tool-call chunks.
            let mut content_buf = String::new();
            let mut accum: BTreeMap<u32, ToolSlot> = BTreeMap::new();

            // Iterate on received frames.
            while let Some(frame) = stream.next().await {
                match frame {
                    Ok(chunk) => {

                        // Chat stream chunk.
                        let Some(choice) = chunk.choices.into_iter().next() else {
                            continue;
                        };

                        // Chat completion delta generated by streamed model responses.
                        let delta = choice.delta;

                        // New token(s) in the answer.
                        if let Some(c) = delta.content {
                            if !c.is_empty() {
                                content_buf.push_str(&c);
                                // return the "thinking" process to the frontend.
                                yield LlmEvent::Token(c);
                            }
                        }

                        // Check if the model wants to call a tool.
                        if let Some(one_tool_call) = delta.tool_calls {
                            // Iterate through the tool calls.
                            for tc in one_tool_call {
                                // Push tool calls to the accumulator.

                                // Get the index slot for inplace edit.
                                let slot = accum.entry(tc.index).or_default();

                                // Push tool call id.
                                if let Some(id) = tc.id {
                                    slot.id = Some(id);
                                }

                                // Push tool call function name.
                                if let Some(f) = tc.function {
                                    if let Some(n) = f.name {
                                        slot.name = Some(n);
                                    }

                                    // push tool call arguments (if any).
                                    if let Some(a) = f.arguments {
                                        // Append the argument tokens to the slot.
                                        slot.arguments.push_str(&a);
                                    }
                                }
                            }
                        }

                        // Terminate the accumulation if stream end successfully.
                        if choice.finish_reason.is_some() {
                            break;
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

            let pending_exec = assemble_tool_calls(accum);

            // No tool calls -> the streamed content was the final answer.
            if pending_exec.is_empty() {
                if !called_any_tool {
                    debug!("model answered without calling any tool (no related tools or history-satisfied)");
                }
                info!(iterations = turn, "Final answer ready");
                guard.completed = true;
                // Done, ending the procedure
                yield LlmEvent::Done;
                return;
            }

            // The model wants tools: any content streamed this turn was a
            // preamble, not the answer, telling buffered consumers to drop it.
            //
            // The frontend will receive a "reset" event and can decide if needed to
            // keep it or not.
            yield LlmEvent::Clear;

            // ──── Tool call ───

            // Record the assistant turn (the API requires the assistant message with
            // its tool_calls to precede the matching tool results), then execute each call.
            called_any_tool = true;
            match build_assistant_message(&content_buf, &pending_exec) {
                Ok(m) => messages.push(m),
                Err(e) => {
                    error!(error = %e, "failed to build assistant message");
                    yield LlmEvent::Error(format!("{e:#}"));
                    return;
                }
            }

            for tc in &pending_exec {
                info!(turn, tool = %tc.name, args = %tc.arguments, "LLM requested tool call");

                // Tool arguments arrive as a JSON *string*, parse to an object.
                let args = parse_tool_arguments(&tc.arguments);

                // Surface failures back to the model (full error chain) rather than aborting, so
                // it can self-correct its arguments next turn.
                let output = match mcp.call_tool_text(&tc.name, args).await {
                    Ok(text) => text,
                    Err(e) => format!("ERROR: tool call failed: {e:#}"),
                };
                info!(turn, tool = %tc.name, bytes = output.len(), "← MCP tool result");

                match build_tool_message(output, tc.id.clone()) {
                    Ok(m) => messages.push(m),
                    Err(e) => {
                        error!(error = %e, "failed to build tool message");
                        yield LlmEvent::Error(format!("{e:#}"));
                        return;
                    }
                };
            }
            // Loop again so the model can use the tool results.
        }

        guard.completed = true;
        yield LlmEvent::Error(format!(
            "agent did not reach a final answer within {MAX_ITERATIONS} iterations"
        ));
    }
}

/// Run the loop to completion and return the full Markdown answer.
///
/// Convenience wrapper over [`agent_stream`] for the non-streaming `/agent`
/// endpoint and the greeting generator: collects every [`LlmEvent::Token`],
/// returns on [`LlmEvent::Done`], and maps [`LlmEvent::Error`] to `Err`.
///
/// # Errors
///
/// Returns `Err` if the loop reports an error event (LLM transport, tool
/// wiring, or non-convergence).
pub async fn generate(
    cfg: GenerationConfig,
    tools: Arc<Vec<ChatCompletionTool>>,
    mcp: McpHandle,
) -> Result<String> {
    // Pin the stream to the heap so it can be dropped and resumed.
    let mut stream = Box::pin(agent_stream(cfg, tools, mcp));

    // Init the string builder
    let mut out = String::new();

    while let Some(ev) = stream.next().await {
        match ev {
            // Append the token to the string builder
            LlmEvent::Token(t) => out.push_str(&t),
            // Drop any preamble streamed during a tool-call turn, keep only
            // the final answer.
            LlmEvent::Clear => out.clear(),
            // Return the final answer
            LlmEvent::Done => return Ok(out),
            LlmEvent::Error(e) => return Err(anyhow!(e)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assemble_tool_calls() {
        let mut accum = BTreeMap::new();
        // Slot with missing fields should be skipped
        accum.insert(
            0,
            ToolSlot {
                id: None,
                name: Some("tool_1".to_string()),
                arguments: "{}".to_string(),
            },
        );
        accum.insert(
            1,
            ToolSlot {
                id: Some("call_2".to_string()),
                name: None,
                arguments: "{}".to_string(),
            },
        );
        // Fully resolved slot
        accum.insert(
            2,
            ToolSlot {
                id: Some("call_3".to_string()),
                name: Some("tool_3".to_string()),
                arguments: "{\"key\": \"val\"}".to_string(),
            },
        );

        let resolved = assemble_tool_calls(accum);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].id, "call_3");
        assert_eq!(resolved[0].name, "tool_3");
        assert_eq!(resolved[0].arguments, "{\"key\": \"val\"}");
    }

    #[test]
    fn test_parse_tool_arguments() {
        let valid = "{\"param\": 123}";
        let parsed = parse_tool_arguments(valid);
        assert_eq!(parsed.get("param"), Some(&serde_json::Value::from(123)));

        let invalid = "not a json";
        let parsed = parse_tool_arguments(invalid);
        assert!(parsed.is_empty());
    }

    #[test]
    fn test_build_assistant_message() {
        // Content only
        let msg = build_assistant_message("hello", &[]).unwrap();
        if let ChatCompletionRequestMessage::Assistant(assistant) = msg {
            match assistant.content {
                Some(
                    async_openai::types::chat::ChatCompletionRequestAssistantMessageContent::Text(
                        text,
                    ),
                ) => {
                    assert_eq!(text, "hello");
                }
                _ => panic!("Expected text content in assistant message"),
            }
            assert!(assistant.tool_calls.is_none());
        } else {
            panic!("Expected assistant message");
        }

        // Tool calls only
        let tool_calls = vec![ResolvedToolCall {
            id: "call_1".to_string(),
            name: "tool_1".to_string(),
            arguments: "{}".to_string(),
        }];
        let msg = build_assistant_message("", &tool_calls).unwrap();
        if let ChatCompletionRequestMessage::Assistant(assistant) = msg {
            assert!(assistant.content.is_none());
            let tcs = assistant.tool_calls.expect("Expected tool calls");
            assert_eq!(tcs.len(), 1);
        } else {
            panic!("Expected assistant message");
        }
    }

    #[test]
    fn test_build_tool_message() {
        let msg = build_tool_message("result text".to_string(), "call_1".to_string()).unwrap();
        if let ChatCompletionRequestMessage::Tool(tool) = msg {
            match tool.content {
                async_openai::types::chat::ChatCompletionRequestToolMessageContent::Text(text) => {
                    assert_eq!(text, "result text");
                }
                _ => panic!("Expected text content in tool message"),
            }
            assert_eq!(tool.tool_call_id, "call_1".to_string());
        } else {
            panic!("Expected tool message");
        }
    }

    #[test]
    fn test_build_request() {
        let cfg = GenerationConfig {
            system: "sys".to_string(),
            user_prompt: "prompt".to_string(),
            history: vec![],
            api_key: "key".to_string(),
            base_url: "url".to_string(),
            model: "model".to_string(),
            app_url: None,
            app_title: None,
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 100,
        };

        // No tools
        let req = build_request(&cfg, vec![], &[]).unwrap();
        assert_eq!(req.model, "model");
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());

        // With tools
        let tools = vec![ChatCompletionTool {
            function: async_openai::types::chat::FunctionObject {
                name: "tool".to_string(),
                description: None,
                parameters: None,
                strict: None,
            },
        }];
        let req = build_request(&cfg, vec![], &tools).unwrap();
        assert_eq!(req.tools.unwrap().len(), 1);
        assert!(req.tool_choice.is_some());
    }
}
