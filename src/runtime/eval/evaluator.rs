//! Evaluation trait skeleton.

use crate::runtime::error::RuntimeResult;

/// One evaluator.
pub trait Evaluator: Send + Sync {
    /// Stable evaluator id.
    fn id(&self) -> &str;

    /// Execute the evaluator.
    fn run(&self) -> RuntimeResult<()>;
}

/// Placeholder evaluator used while concrete eval modes are implemented.
#[derive(Debug, Clone)]
pub struct NoopEvaluator {
    id: String,
}

impl NoopEvaluator {
    /// Create a no-op evaluator with a stable id.
    pub fn new(id: String) -> Self {
        Self { id }
    }
}

impl Evaluator for NoopEvaluator {
    fn id(&self) -> &str {
        &self.id
    }

    fn run(&self) -> RuntimeResult<()> {
        let _ = &self.id;
        Ok(())
    }
}
