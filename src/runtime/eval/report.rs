//! Eval report types.

/// Eval summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalReport {
    /// Number of passing cases.
    pub passed: usize,
    /// Number of failing cases.
    pub failed: usize,
    /// Total observed latency in milliseconds, when available.
    pub latency_ms: u64,
    /// Total observed tokens, when available.
    pub tokens: u64,
    /// Number of refusal responses.
    pub refusals: usize,
    /// Number of fallback responses.
    pub fallbacks: usize,
    /// Baseline or artifact provenance.
    pub provenance: Option<String>,
    /// Response eval regressions or case-level failure reasons.
    pub regressions: Vec<String>,
}
