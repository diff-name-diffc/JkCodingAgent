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
}
