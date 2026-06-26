//! Shared runtime schema.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::History;

/// One raw user turn entering the runtime.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentTurnInput {
    /// Runtime request id.
    pub request_id: Uuid,
    /// Prompt sent to the agent (may be memory-augmented during the turn).
    pub prompt: String,
    /// Original user input, preserved across memory augmentation so memory
    /// summaries record the raw turn rather than the augmented prompt.
    pub raw_input: String,
    /// Client-provided history fallback.
    pub history: Vec<History>,
    /// Optional server-side memory session id.
    pub session_id: Option<String>,
    /// Optional frontend option id.
    pub option_id: Option<String>,
}

/// Warning emitted by deterministic normalization or policy stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeWarning {
    /// Machine-readable warning code.
    pub code: String,
    /// Human-readable warning message.
    pub message: String,
}

/// Slots extracted from a user prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedSlots {
    /// Optional time range phrase.
    pub time_range: Option<String>,
    /// Optional metric id.
    pub metric: Option<String>,
    /// Optional asset id or name.
    pub asset: Option<String>,
    /// Optional rank limit.
    pub rank_limit: Option<u32>,
}

/// Source that selected the final intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntentSource {
    /// Frontend option id path.
    OptionPath,
    /// Rule/lexicon classifier.
    RuleLexicon,
    /// Text classifier overrode an option path.
    TextOverride,
    /// Unknown fallback.
    Unknown,
}

/// Deterministically normalized input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedInput {
    /// Normalized prompt text.
    pub prompt: String,
    /// Selected intent id.
    pub intent: String,
    /// Intent confidence.
    pub confidence: f32,
    /// Candidate intent ids observed while classifying.
    pub candidate_intents: Vec<String>,
    /// Source of the selected intent.
    pub intent_source: Option<IntentSource>,
    /// Extracted slots.
    pub slots: NormalizedSlots,
    /// Non-fatal warnings.
    pub warnings: Vec<RuntimeWarning>,
}

/// Internal turn frames used by the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentTurnFrame {
    /// A token fragment.
    Token { data: String },
    /// Clear any accumulated answer buffer.
    Clear,
    /// Tool call metadata, internal only.
    ToolCalled {
        /// Tool name.
        name: String,
        /// SHA-256 hash of tool arguments.
        args_hash: String,
    },
    /// Tool result metadata, internal only.
    ToolResult {
        /// Tool name.
        name: String,
        /// Result byte length.
        bytes: usize,
        /// Whether the tool call succeeded.
        ok: bool,
    },
    /// Completed normally.
    Done,
    /// Terminal error.
    Error { data: String },
}
