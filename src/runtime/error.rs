//! Runtime error model.

use thiserror::Error;

/// Error type used inside the runtime core.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// Boot-time or configuration contract error.
    #[error("runtime config error: {0}")]
    Config(String),
    /// Config referenced a module id that has no registered implementation.
    #[error("unknown module id `{id}` in [{section}]")]
    UnknownModule {
        /// Unknown module id.
        id: String,
        /// Config section that referenced the id.
        section: String,
    },
    /// Config referenced an intent id outside the allowlist.
    #[error("intent `{0}` not in allowlist")]
    IntentNotAllowed(String),
    /// Request validation or policy error.
    #[error("runtime request error: {0}")]
    Request(String),
    /// Input was required but absent.
    #[error("input required")]
    InputRequired,
    /// Input exceeded configured character limit.
    #[error("input too long: {0} chars")]
    InputTooLong(usize),
    /// Pipeline contract was violated.
    #[error("pipeline contract invalid")]
    PipelineContract,
    /// Audit sink failed.
    #[error("audit sink failed: {0}")]
    AuditSink(String),
    /// Upstream agent, tool, or model error.
    #[error("runtime upstream error: {0}")]
    Upstream(String),
    /// Internal runtime failure.
    #[error("runtime internal error: {0}")]
    Internal(String),
}

/// Runtime result alias.
pub type RuntimeResult<T> = Result<T, RuntimeError>;
