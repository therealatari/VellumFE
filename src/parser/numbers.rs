//! Numeric extraction helpers for progress-bar style text.

/// Parse numeric current/max out of a progress bar text string.
/// Supports:
/// - "label 324/326" -> (324, 326)
/// - "324/326" -> (324, 326)
/// - "label (100%)" or "label 100%" -> (100, 100)
/// - "label" -> (percentage, 100)
pub(crate) fn parse_progress_numbers(text: &str, percentage: u32) -> (u32, u32) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (percentage, 100);
    }

    // Slash form: current/max
    if let Some(slash_pos) = trimmed.rfind('/') {
        let before_slash = &trimmed[..slash_pos];
        let after_slash = &trimmed[slash_pos + 1..];

        let current = last_number(before_slash).unwrap_or(percentage);
        let maximum = first_number(after_slash).unwrap_or(100);
        return (current, maximum);
    }

    // Percent or single number form: treat as current, max = 100
    if let Some(num) = first_number(trimmed) {
        return (num, 100);
    }

    // Label-only: fall back to percentage/max
    (percentage, 100)
}

pub(crate) fn first_number(input: &str) -> Option<u32> {
    input
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '%')
        .find_map(|token| {
            token
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })
}

pub(crate) fn last_number(input: &str) -> Option<u32> {
    input
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == '%')
        .rev()
        .find_map(|token| {
            token
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })
}
