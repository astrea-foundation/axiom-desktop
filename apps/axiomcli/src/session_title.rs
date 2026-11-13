//! Bounded, local labels for a conversation's first accepted prompt.

/// Derive a display title without a provider request or background task.
#[must_use]
pub fn session_title(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let safe_line = line
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let line = safe_line.trim().trim_matches(|character: char| {
        matches!(character, '"' | '\'' | '`' | '#' | '*' | '_' | '[' | ']')
    });
    let mut title = line
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    let mut boundary = title.len().min(80);
    while !title.is_char_boundary(boundary) {
        boundary -= 1;
    }
    title.truncate(boundary);
    let title = title
        .trim()
        .trim_end_matches(['.', ',', ':', ';', '!', '?', '-', '—'])
        .trim();
    if title.is_empty() {
        "Conversation".into()
    } else {
        title.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::session_title;

    #[test]
    fn labels_use_the_first_nonempty_line_and_at_most_eight_words() {
        assert_eq!(
            session_title("\n  \"Repair the workspace.\"\nExtra context"),
            "Repair the workspace"
        );
        assert_eq!(
            session_title("Please repair the workspace without changing its theme today"),
            "Please repair the workspace without changing its theme"
        );
    }

    #[test]
    fn labels_are_bounded_valid_utf8_without_control_characters() {
        let title = session_title(&format!("\u{1b}{}", "界".repeat(40)));
        assert_eq!(title, "界".repeat(26));
        assert!(title.len() <= 80);
        assert_eq!(session_title("\t\n***?!***"), "Conversation");
        assert_eq!(session_title(""), "Conversation");
    }
}
