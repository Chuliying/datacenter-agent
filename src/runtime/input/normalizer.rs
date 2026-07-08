//! Prompt text normalization.

use unicode_normalization::UnicodeNormalization;

/// Normalize Unicode text with NFKC and trimmed whitespace.
pub fn normalize_text(input: &str) -> String {
    let mapped: String = input
        .nfkc()
        .flat_map(|ch| map_cjk_punctuation(ch).to_lowercase())
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn map_cjk_punctuation(ch: char) -> char {
    match ch {
        '「' | '」' | '『' | '』' | '“' | '”' | '‘' | '’' => '"',
        '、' | '，' => ',',
        '。' => '.',
        '？' => '?',
        '！' => '!',
        '：' => ':',
        '；' => ';',
        '（' => '(',
        '）' => ')',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_fullwidth_and_cjk_punctuation() {
        let normalized = normalize_text(" 「近６個月」、營收？ ");

        assert_eq!(normalized, "\"近6個月\",營收?");
    }

    #[test]
    fn collapses_whitespace_and_lowercases_ascii() {
        let normalized = normalize_text(" Revenue \n\t TREND ");

        assert_eq!(normalized, "revenue trend");
    }
}
