//! Deterministic summary-field sanitization (PRD FR-002).
//!
//! Fixed order, one pipeline: redact configured sensitive patterns →
//! normalize whitespace → truncate to the per-field character limit.
//! The repository sanitizes caller-supplied fields; it cannot verify a field
//! is a genuine summary (that boundary is FU-006, the integration slice).

use regex::Regex;

/// Replacement inserted where a sensitive pattern matched.
pub const REDACTED: &str = "[REDACTED]";

/// Default sensitive patterns: email, IPv4, cookie pairs, bearer tokens
/// (PRD: "redact configured sensitive patterns (email, IP, cookie, token)").
pub fn default_redact_patterns() -> Vec<Regex> {
    [
        // email
        r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}",
        // IPv4
        r"\b(?:\d{1,3}\.){3}\d{1,3}\b",
        // bearer token
        r"(?i)bearer\s+[a-z0-9._~+/=-]+",
        // cookie header with its pairs
        r"(?i)\bcookie\s*:\s*[^\s;]+(?:;\s*[^\s;]+)*",
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).expect("default redact pattern must compile"))
    .collect()
}

/// Apply the fixed sanitization pipeline to one summary field.
pub fn sanitize_field(input: &str, patterns: &[Regex], char_limit: usize) -> String {
    let mut redacted = input.to_string();
    for pattern in patterns {
        redacted = pattern.replace_all(&redacted, REDACTED).into_owned();
    }
    let normalized = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    normalized.chars().take(char_limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sanitize(input: &str) -> String {
        sanitize_field(input, &default_redact_patterns(), 500)
    }

    /// AC-003 fixtures: none of the raw sensitive values survive.
    #[test]
    fn redacts_email() {
        let out = sanitize("contact alice@example.com for access");
        assert!(!out.contains("alice@example.com"), "got: {out}");
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redacts_ipv4() {
        let out = sanitize("peer was 203.0.113.9 today");
        assert!(!out.contains("203.0.113.9"), "got: {out}");
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redacts_bearer_token() {
        let out = sanitize("sent Authorization: Bearer sk-live-abc123 upstream");
        assert!(!out.contains("sk-live-abc123"), "got: {out}");
        assert!(out.contains(REDACTED));
    }

    #[test]
    fn redacts_cookie_pair() {
        let out = sanitize("with Cookie: session=deadbeefcafe; theme=dark");
        assert!(!out.contains("deadbeefcafe"), "got: {out}");
        assert!(out.contains(REDACTED));
    }

    /// Whitespace runs collapse to one space, ends trimmed.
    #[test]
    fn normalizes_whitespace() {
        assert_eq!(sanitize("  a\t\tb\n\nc  "), "a b c");
    }

    /// Truncation is by characters, after redaction, and never panics on
    /// multi-byte boundaries.
    #[test]
    fn truncates_to_char_limit() {
        let long = "月".repeat(600);
        let out = sanitize_field(&long, &default_redact_patterns(), 500);
        assert_eq!(out.chars().count(), 500);
    }

    /// TC-B05: exactly the limit passes untouched; one char over truncates.
    #[test]
    fn char_limit_boundary_is_exact() {
        let exact = "字".repeat(500);
        assert_eq!(sanitize_field(&exact, &[], 500), exact);
        let over = "字".repeat(501);
        assert_eq!(sanitize_field(&over, &[], 500).chars().count(), 500);
    }

    /// Order is redact-first: a token pushed past the limit by earlier text
    /// must still be redacted, not truncated into surviving plaintext.
    #[test]
    fn redacts_before_truncating() {
        let input = format!("{} Bearer sk-secret-value", "x".repeat(495));
        let out = sanitize_field(&input, &default_redact_patterns(), 500);
        assert!(!out.contains("sk-secret-value"), "got: {out}");
    }
}
