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
