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

//! The streaming event model: one injected [`EventSink`] carrying one tagged [`AgentEvent`].
//!
//! Streaming an agent's *thinking + tool-using process* to the user is modelled as an **effect
//! sink**, not an LLM feature (plan §8.2). Three boundaries emit onto one ordered sink:
//!
//! - the **streaming LLM adapter** ([`StreamingOpenAiLlm`](crate::agent::llm::StreamingOpenAiLlm))
//!   — content / reasoning deltas and the model's tool-call *intent*;
//! - the **tool wrapper** ([`StreamingTool`](crate::agent::tools::StreamingTool)) — a tool
//!   starting, producing, or rejecting;
//! - the **orchestrator** ([`Orchestrator::run_emitting`](crate::agent::engine::Orchestrator)) —
//!   pipeline stage transitions, emitted from *outside* [`SubAgent::run`](crate::agent::engine::SubAgent),
//!   so the normative `run(payload) -> Result<payload>` morphism is unchanged.
//!
//! This is the same idiom as the runtime's `TurnEmit` sink, one layer down: a no-op
//! ([`NullSink`]) serves the buffered path and every unit test, while [`ChannelSink`] forwards to
//! SSE.
//!
//! # References
//!
//! - Sub-agent plan §8 — the streaming event architecture (Path A)

#![allow(dead_code)] // groundwork: emitted by the streaming path, drained once wired behind AgentPort.

use crate::agent::config::SubAgentId;
use crate::agent::payload::{ArtifactKey, PayloadKind};

/// The outcome of a completed stage — the signal a UI turns into a success/failure indicator
/// (e.g. a green vs. red dot next to the sub-agent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageOutcome {
    /// The stage's `run` returned `Ok`.
    Success,
    /// The stage's `run` returned `Err`; the pipeline aborts after this, so a terminal
    /// [`AgentEvent::Error`] always follows.
    Failure,
}

/// One event on the sub-agent layer's ordered stream.
///
/// A superset of the monolith's `LlmEvent`: it keeps content / tool / done framing and adds a
/// distinct reasoning channel, per-stage framing, and artifact-keyed tool results. The
/// `PipelineAgentPort` is the single point that maps these down to the runtime's user-facing
/// `TurnEvent`s (plan §8.2 / §9); internal variants stay audit-only on the wire.
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    // ── pipeline framing — emitted by the Orchestrator, from OUTSIDE run() ──
    /// A stage began, handed a payload of this kind.
    StageStarted {
        /// The stage that began.
        agent: SubAgentId,
        /// The payload kind the stage was handed.
        input: PayloadKind,
    },
    /// A stage added these artifact keys (newly present versus its input).
    StageProduced {
        /// The stage that produced them.
        agent: SubAgentId,
        /// The keys produced this stage, in sorted order.
        keys: Vec<ArtifactKey>,
    },
    /// A stage finished — `outcome` says whether its `run` succeeded or failed, so a consumer can
    /// render a success/failure indicator. A `Failure` is always followed by a terminal
    /// [`AgentEvent::Error`].
    StageFinished {
        /// The stage that finished.
        agent: SubAgentId,
        /// Whether the stage succeeded or failed.
        outcome: StageOutcome,
    },

    // ── llm turn — emitted by the streaming adapter ──
    /// A fragment of the model's reasoning/thinking (its own channel; unemitted until productized).
    ReasoningDelta {
        /// The reasoning text fragment.
        text: String,
    },
    /// A fragment of the model's answer content.
    ContentDelta {
        /// The content text fragment.
        text: String,
    },
    /// A raw fragment of a tool call's streamed JSON arguments (optional live sugar).
    ToolArgsDelta {
        /// The tool-call id the fragment belongs to.
        id: String,
        /// The raw argument fragment (partial JSON string).
        fragment: String,
    },
    /// The model finished proposing a tool call (arguments assembled).
    ToolCallProposed {
        /// The tool-call id.
        id: String,
        /// The advertised tool name.
        name: String,
    },

    // ── tool execution — emitted by the StreamingTool wrapper ──
    /// A granted tool began executing.
    ToolStarted {
        /// The advertised tool name.
        name: String,
    },
    /// A tool produced a value into `target`.
    ToolProduced {
        /// The advertised tool name.
        name: String,
        /// The artifact slot the value filled.
        target: ArtifactKey,
    },
    /// A tool rejected the call (retryable — fed back to the model, no artifact recorded).
    ToolRejected {
        /// The advertised tool name.
        name: String,
        /// The rejection reason surfaced to the model.
        reason: String,
    },

    // ── terminal ──
    /// The run finished; carries the final user-facing answer.
    Finished {
        /// The terminal stage's assistant text.
        assistant: String,
    },
    /// The run failed. A *deliverable* frame — never merely a dropped connection.
    Error {
        /// The error message.
        message: String,
    },
}

/// The injected sink: sync, fire-and-forget (mirroring the runtime's `TurnEmit`).
///
/// Emission must never block the agent, so implementations drop rather than await.
pub trait EventSink: Send + Sync {
    /// Emits one event. Fire-and-forget: a slow or absent consumer must not stall the agent.
    fn emit(&self, event: AgentEvent);
}

/// The buffered / non-streaming / unit-test sink: drops every event.
///
/// This is the null object that keeps *one* code path — a buffered stage and a scripted test both
/// run against it and behave identically to a streaming run minus the emissions.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: AgentEvent) {}
}

/// The SSE sink: forwards each event onto a bounded channel.
///
/// `try_send` drops on a full buffer so a slow client never stalls token generation; swap for an
/// unbounded channel or a spawned drainer if lossless delivery is ever required (plan §8.2).
pub struct ChannelSink(pub tokio::sync::mpsc::Sender<AgentEvent>);

impl EventSink for ChannelSink {
    fn emit(&self, event: AgentEvent) {
        let _ = self.0.try_send(event);
    }
}

/// Test-only sinks, shared across the crate's unit tests.
#[cfg(test)]
pub(crate) mod test_support {
    use super::{AgentEvent, EventSink};
    use std::sync::Mutex;

    /// A sink that records every event, so a unit test can assert the exact emitted sequence.
    #[derive(Default)]
    pub(crate) struct CollectingSink {
        events: Mutex<Vec<AgentEvent>>,
    }

    impl CollectingSink {
        /// Creates an empty collecting sink.
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Returns a snapshot of the events recorded so far, in emission order.
        pub(crate) fn events(&self) -> Vec<AgentEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl EventSink for CollectingSink {
        fn emit(&self, event: AgentEvent) {
            self.events.lock().unwrap().push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::CollectingSink;
    use super::*;

    #[test]
    fn null_sink_drops_every_event() {
        let sink = NullSink;
        // No panic, no state — the point is that it is a total no-op.
        sink.emit(AgentEvent::ContentDelta { text: "x".into() });
        sink.emit(AgentEvent::Finished {
            assistant: "y".into(),
        });
    }

    #[test]
    fn collecting_sink_records_in_emission_order() {
        let sink = CollectingSink::new();
        sink.emit(AgentEvent::ToolStarted {
            name: "bill_revenue".into(),
        });
        sink.emit(AgentEvent::ContentDelta { text: "hi".into() });
        assert_eq!(
            sink.events(),
            vec![
                AgentEvent::ToolStarted {
                    name: "bill_revenue".into()
                },
                AgentEvent::ContentDelta { text: "hi".into() },
            ]
        );
    }

    #[test]
    fn channel_sink_forwards_then_drops_on_a_full_buffer() {
        // A bounded channel of 1: the first event is delivered, the second is silently dropped
        // (emit never blocks). Constructing the channel + try_send/try_recv need no runtime.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(1);
        let sink = ChannelSink(tx);
        sink.emit(AgentEvent::ContentDelta { text: "a".into() });
        sink.emit(AgentEvent::ContentDelta { text: "b".into() }); // dropped: buffer full

        assert_eq!(
            rx.try_recv().unwrap(),
            AgentEvent::ContentDelta { text: "a".into() }
        );
        assert!(rx.try_recv().is_err(), "second event must have been dropped");
    }
}
