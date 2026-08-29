//! Small shared helpers.

/// Truncate `s` to at most `max` characters, adding an ellipsis when cut.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_short_strings() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
    }

    #[test]
    fn cuts_long_strings_with_ellipsis() {
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn counts_chars_not_bytes() {
        // 5 multi-byte chars, limit 5 -> unchanged
        assert_eq!(truncate("café☕é", 6), "café☕é");
    }
}
