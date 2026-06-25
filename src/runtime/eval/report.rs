//! Eval report skeleton.

/// Eval summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EvalReport {
    /// Number of passing cases.
    pub passed: usize,
    /// Number of failing cases.
    pub failed: usize,
}
