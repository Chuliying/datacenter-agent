//! Optional LLM-backed input normalization seam.

use async_trait::async_trait;

use super::error::RuntimeResult;
use super::schema::NormalizedInput;

/// Optional async normalizer invoked after deterministic input stages.
#[async_trait]
pub trait LlmInputNormalizer: Send + Sync {
    /// Enhance or rewrite normalized input.
    async fn normalize(&self, input: NormalizedInput) -> RuntimeResult<NormalizedInput>;
}

/// Disabled normalizer that returns input unchanged.
#[derive(Debug, Default)]
pub struct DisabledLlmNormalizer;

#[async_trait]
impl LlmInputNormalizer for DisabledLlmNormalizer {
    async fn normalize(&self, input: NormalizedInput) -> RuntimeResult<NormalizedInput> {
        Ok(input)
    }
}
