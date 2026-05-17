pub fn truncate_for_display(text: &str, max_chars: usize, suffix: &str) -> String {
    if max_chars == 0 {
        return suffix.to_string();
    }

    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => {
            let mut truncated = text[..idx].to_string();
            truncated.push_str(suffix);
            truncated
        }
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::truncate_for_display;

    #[test]
    fn keeps_short_text_unchanged() {
        assert_eq!(truncate_for_display("hello", 10, "..."), "hello");
    }

    #[test]
    fn truncates_multibyte_text_without_panicking() {
        assert_eq!(truncate_for_display("中文编辑器", 3, "..."), "中文编...");
    }

    #[test]
    fn supports_zero_limit() {
        assert_eq!(truncate_for_display("中文", 0, "..."), "...");
    }

    // --- Additional tests ---

    #[test]
    fn exact_length_text_not_truncated() {
        assert_eq!(truncate_for_display("abc", 3, "..."), "abc");
    }

    #[test]
    fn truncates_at_exact_boundary() {
        assert_eq!(truncate_for_display("abcd", 3, "..."), "abc...");
    }

    #[test]
    fn empty_string_with_any_limit() {
        assert_eq!(truncate_for_display("", 5, "..."), "");
    }

    #[test]
    fn empty_string_with_zero_limit() {
        assert_eq!(truncate_for_display("", 0, "..."), "...");
    }

    #[test]
    fn empty_suffix() {
        assert_eq!(truncate_for_display("hello world", 5, ""), "hello");
    }

    #[test]
    fn empty_suffix_with_zero_limit() {
        assert_eq!(truncate_for_display("hello", 0, ""), "");
    }

    #[test]
    fn unicode_suffix() {
        assert_eq!(truncate_for_display("hello world", 5, "…"), "hello…");
    }

    #[test]
    fn long_suffix_added() {
        assert_eq!(truncate_for_display("abcdefghij", 3, " [truncated]"), "abc [truncated]");
    }

    #[test]
    fn single_char_limit() {
        assert_eq!(truncate_for_display("hello", 1, "!"), "h!");
    }

    #[test]
    fn single_char_input_truncated() {
        assert_eq!(truncate_for_display("x", 0, "..."), "...");
    }

    #[test]
    fn single_char_input_fits() {
        assert_eq!(truncate_for_display("x", 1, "..."), "x");
    }

    #[test]
    fn truncates_mixed_ascii_and_cjk() {
        assert_eq!(truncate_for_display("abc中文xyz", 5, "..."), "abc中文...");
    }

    #[test]
    fn limit_one_with_cjk() {
        assert_eq!(truncate_for_display("中文", 1, "..."), "中...");
    }

    #[test]
    fn very_large_limit() {
        let text = "short";
        assert_eq!(truncate_for_display(text, 10000, "..."), "short");
    }

    #[test]
    fn newline_characters_count_as_chars() {
        assert_eq!(truncate_for_display("a\nb\nc", 3, "..."), "a\nb...");
    }

    #[test]
    fn emoji_handling() {
        assert_eq!(truncate_for_display("😀😃😄😁", 2, "..."), "😀😃...");
    }

    #[test]
    fn combining_characters_preserved() {
        // e + combining acute accent is two chars (scalar values) but one grapheme.
        // truncate_for_display counts scalar values, not graphemes.
        // "e\u{0301}e\u{0301}e\u{0301}" has 6 scalar values; limit 2 takes "e\u{0301}".
        let text = "e\u{0301}e\u{0301}e\u{0301}";
        assert_eq!(truncate_for_display(text, 2, "."), "e\u{0301}.");
    }

    #[test]
    fn preserves_original_when_under_limit() {
        let text = "hello";
        let result = truncate_for_display(text, 100, "...");
        assert_eq!(result, "hello");
        // Should not contain suffix when not truncated
        assert!(!result.contains("..."));
    }
}
