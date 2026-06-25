//! Response baseline loading skeleton.

/// Baseline provenance status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineStatus {
    /// Baseline is intentionally not available yet.
    Pending,
    /// Baseline was loaded from a replay artifact.
    Loaded,
}
