//! Structural input guardrails.

use crate::runtime::error::{RuntimeError, RuntimeResult};

/// Validate prompt presence and length.
pub fn validate_prompt(prompt: &str, max_prompt_chars: usize) -> RuntimeResult<()> {
    if prompt.trim().is_empty() {
        return Err(RuntimeError::InputRequired);
    }

    let char_count = prompt.chars().count();
    if char_count > max_prompt_chars {
        return Err(RuntimeError::InputTooLong(char_count));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_prompt_at_runtime_limit() {
        let prompt = "x".repeat(4_000);

        assert!(validate_prompt(&prompt, 4_000).is_ok());
    }

    #[test]
    fn rejects_prompt_over_runtime_limit() {
        let prompt = "x".repeat(4_001);

        let err = validate_prompt(&prompt, 4_000).expect_err("prompt over cap should fail");

        assert!(matches!(err, RuntimeError::InputTooLong(4_001)));
    }

    #[test]
    fn accepts_approved_2001_char_parity_diff() {
        let prompt = "x".repeat(2_001);

        assert!(validate_prompt(&prompt, 4_000).is_ok());
    }
}
