//! Terminal-safe text and display-width helpers.

use std::time::Duration;

use ratatui::layout::{Constraint, Layout, Rect};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(super) fn char_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(0)
}

/// Word-wraps to a display width, hard-splitting words that cannot fit on a
/// line of their own. Measuring in display columns keeps wide glyphs such as
/// emoji from pushing text one cell past the right edge.
pub(super) fn wrap_to_width(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let expanded = line.replace('\t', "    ");
    let mut wrapped: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0_usize;
    let mut started = false;
    for word in expanded.split(' ') {
        for (piece_index, piece) in split_to_width(word, width).into_iter().enumerate() {
            let piece_width = display_width(&piece);
            let separator = usize::from(piece_index == 0 && started);
            if started && current_width + separator + piece_width > width {
                wrapped.push(std::mem::take(&mut current));
                current_width = 0;
            } else if separator == 1 {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(&piece);
            current_width += piece_width;
            started = true;
        }
    }
    wrapped.push(current);
    wrapped
}

/// Breaks a single word into chunks no wider than `width` display columns.
pub(super) fn split_to_width(word: &str, width: usize) -> Vec<String> {
    if display_width(word) <= width {
        return vec![word.to_owned()];
    }
    let mut pieces = Vec::new();
    let mut chunk = String::new();
    let mut chunk_width = 0_usize;
    for character in word.chars() {
        let next = char_width(character);
        if chunk_width + next > width && !chunk.is_empty() {
            pieces.push(std::mem::take(&mut chunk));
            chunk_width = 0;
        }
        chunk.push(character);
        chunk_width += next;
    }
    if !chunk.is_empty() {
        pieces.push(chunk);
    }
    pieces
}

/// Keeps the trailing `width` display columns of `text`, so a composer line
/// longer than the field scrolls with the caret instead of hiding it.
pub(super) fn tail_to_width(text: &str, width: usize) -> String {
    let mut remaining = display_width(text);
    if remaining <= width {
        return text.to_owned();
    }
    let mut start = text.len();
    for (index, character) in text.char_indices() {
        if remaining <= width {
            start = index;
            break;
        }
        remaining -= char_width(character);
        start = index + character.len_utf8();
    }
    text[start..].to_owned()
}

/// Keeps the leading display columns of `text`, reserving the final column for
/// an ellipsis when truncation is required.
pub(super) fn truncate_end(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let mut truncated = String::new();
    let mut used = 0_usize;
    for character in text.chars() {
        let character_width = char_width(character);
        if used.saturating_add(character_width) > width - 1 {
            break;
        }
        truncated.push(character);
        used = used.saturating_add(character_width);
    }
    truncated.push('…');
    truncated
}

/// Centres a panel of an exact row count, clamped to the available height.
pub(super) fn fitted_rect(width_percent: u16, rows: u16, area: Rect) -> Rect {
    let height = rows.min(area.height.saturating_sub(2)).max(3);
    let outer = centered_rect(width_percent, 100, area);
    Rect {
        x: outer.x,
        y: outer.y + (outer.height.saturating_sub(height)) / 2,
        width: outer.width,
        height,
    }
}

pub(super) fn centered_rect(width_percent: u16, height_percent: u16, area: Rect) -> Rect {
    let vertical = Layout::vertical([
        Constraint::Percentage((100 - height_percent) / 2),
        Constraint::Percentage(height_percent),
        Constraint::Percentage((100 - height_percent) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - width_percent) / 2),
        Constraint::Percentage(width_percent),
        Constraint::Percentage((100 - width_percent) / 2),
    ])
    .split(vertical[1])[1]
}

pub(super) fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        return format!("Worked for {seconds}s");
    }
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if remainder == 0 {
        format!("Worked for {minutes}m")
    } else {
        format!("Worked for {minutes}m {remainder}s")
    }
}

#[must_use]
pub fn sanitize_terminal_text(input: &str) -> String {
    input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

pub(super) fn user_facing_provider_error(message: &str) -> String {
    let normalized = message.to_ascii_lowercase();
    let payment_required = normalized.contains("http 402")
        || normalized.contains("402 payment required")
        || normalized.contains("payment required");
    let insufficient_credit = normalized.contains("insufficient_credit")
        || normalized.contains("insufficient credit")
        || normalized.contains("out of credit")
        || normalized.contains("top up your axiom balance");
    if is_attestation_failure(&normalized) {
        "Secure connection could not be verified\nYour request was not sent. Run /refresh to try again or /security for details."
            .into()
    } else if payment_required && insufficient_credit {
        "Out of credits\nUse /topup for a Zcash deposit or /redeem for a gift code. Check credit with /balance.".into()
    } else {
        message.to_owned()
    }
}

pub(super) fn is_attestation_failure(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("attestationrejected")
        || normalized.contains("attestation rejected")
        || normalized.contains("attestationunavailable")
        || normalized.contains("attestation unavailable")
}

pub(super) fn truncate_middle(input: &str, max_chars: usize) -> String {
    let characters: Vec<_> = input.chars().collect();
    if characters.len() <= max_chars {
        return input.into();
    }
    if max_chars <= 1 {
        return "…".into();
    }
    let left = (max_chars - 1) / 2;
    let right = max_chars - left - 1;
    format!(
        "{}…{}",
        characters[..left].iter().collect::<String>(),
        characters[characters.len() - right..]
            .iter()
            .collect::<String>()
    )
}
