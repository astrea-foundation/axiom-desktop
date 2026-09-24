use std::{
    cell::{Cell, RefCell},
    ops::Deref,
};

use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr;

const TAB_WIDTH: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
struct VisualLine {
    source_start: usize,
    source_end: usize,
    display_end: usize,
}

#[derive(Clone, Debug)]
struct WrapCache {
    width: u16,
    lines: Vec<VisualLine>,
}

#[derive(Clone, Copy)]
struct SourceGrapheme {
    start: usize,
    end: usize,
    width: usize,
    whitespace: bool,
}

/// Editable prompt state kept separately from the TUI's layout and rendering.
///
/// Byte offsets are always UTF-8 grapheme boundaries. Wrapping produces source
/// ranges rather than a second copy of the prompt, so rendering and navigation
/// agree about where every visible row begins and ends.
#[derive(Debug, Default)]
pub(super) struct ComposerBuffer {
    text: String,
    cursor: usize,
    preferred_column: Option<usize>,
    wrap_cache: RefCell<Option<WrapCache>>,
    viewport_scroll: Cell<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ComposerView {
    pub lines: Vec<String>,
    pub cursor_row: u16,
    pub cursor_column: u16,
    pub scroll: usize,
    pub total_lines: usize,
}

impl ComposerBuffer {
    pub fn as_str(&self) -> &str {
        &self.text
    }

    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_cursor_at_end(&self) -> bool {
        self.cursor == self.text.len()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
        self.after_edit();
    }

    pub fn take_text(&mut self) -> String {
        self.cursor = 0;
        self.preferred_column = None;
        self.viewport_scroll.set(0);
        self.wrap_cache.borrow_mut().take();
        std::mem::take(&mut self.text)
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.after_edit();
    }

    pub fn insert_char(&mut self, character: char) {
        self.text.insert(self.cursor, character);
        self.cursor += character.len_utf8();
        self.after_edit();
    }

    pub fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.after_edit();
    }

    pub fn backspace(&mut self) -> bool {
        let Some((previous, _)) = self.text[..self.cursor].grapheme_indices(true).next_back()
        else {
            return false;
        };
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        self.after_edit();
        true
    }

    pub fn delete_forward(&mut self) -> bool {
        let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() else {
            return false;
        };
        self.text
            .replace_range(self.cursor..self.cursor + grapheme.len(), "");
        self.after_edit();
        true
    }

    pub fn move_left(&mut self) -> bool {
        let Some((previous, _)) = self.text[..self.cursor].grapheme_indices(true).next_back()
        else {
            return false;
        };
        self.cursor = previous;
        self.preferred_column = None;
        true
    }

    pub fn move_right(&mut self) -> bool {
        let Some(grapheme) = self.text[self.cursor..].graphemes(true).next() else {
            return false;
        };
        self.cursor += grapheme.len();
        self.preferred_column = None;
        true
    }

    pub fn move_to_line_start(&mut self) -> bool {
        let start = self.text[..self.cursor]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let changed = start != self.cursor;
        self.cursor = start;
        self.preferred_column = None;
        changed
    }

    pub fn move_to_document_start(&mut self) -> bool {
        let changed = self.cursor != 0;
        self.cursor = 0;
        self.preferred_column = None;
        changed
    }

    pub fn move_to_line_end(&mut self) -> bool {
        let end = self.text[self.cursor..]
            .find('\n')
            .map_or(self.text.len(), |offset| self.cursor + offset);
        let changed = end != self.cursor;
        self.cursor = end;
        self.preferred_column = None;
        changed
    }

    pub fn move_to_document_end(&mut self) -> bool {
        let changed = self.cursor != self.text.len();
        self.cursor = self.text.len();
        self.preferred_column = None;
        changed
    }

    pub fn move_up(&mut self, width: u16) -> bool {
        self.move_vertical(width, -1)
    }

    pub fn move_down(&mut self, width: u16) -> bool {
        self.move_vertical(width, 1)
    }

    pub fn desired_height(&self, width: u16) -> u16 {
        u16::try_from(self.wrapped_lines(width).len()).unwrap_or(u16::MAX)
    }

    pub fn view(&self, width: u16, height: u16) -> ComposerView {
        let width = width.max(1);
        let height = usize::from(height.max(1));
        let lines = self.wrapped_lines(width);
        let (cursor_row, cursor_column) = cursor_position(&self.text, &lines, width, self.cursor);
        let max_scroll = lines.len().saturating_sub(height);
        let mut scroll = self.viewport_scroll.get().min(max_scroll);
        if cursor_row < scroll {
            scroll = cursor_row;
        } else if cursor_row >= scroll + height {
            scroll = cursor_row + 1 - height;
        }
        self.viewport_scroll.set(scroll);

        let visible = lines
            .iter()
            .skip(scroll)
            .take(height)
            .map(|line| display_slice(&self.text[line.source_start..line.display_end]))
            .collect();
        ComposerView {
            lines: visible,
            cursor_row: u16::try_from(cursor_row.saturating_sub(scroll)).unwrap_or(u16::MAX),
            cursor_column: u16::try_from(cursor_column).unwrap_or(u16::MAX),
            scroll,
            total_lines: lines.len(),
        }
    }

    fn move_vertical(&mut self, width: u16, direction: isize) -> bool {
        let lines = self.wrapped_lines(width.max(1));
        let (row, column) = cursor_position(&self.text, &lines, width.max(1), self.cursor);
        let preferred = self.preferred_column.unwrap_or(column);
        let target = if direction.is_negative() {
            row.checked_sub(direction.unsigned_abs())
        } else {
            row.checked_add(direction.unsigned_abs())
                .filter(|target| *target < lines.len())
        };
        let Some(target) = target else {
            return false;
        };
        self.cursor = byte_at_column(&self.text, &lines[target], preferred);
        self.preferred_column = Some(preferred);
        true
    }

    fn wrapped_lines(&self, width: u16) -> Vec<VisualLine> {
        let width = width.max(1);
        let needs_rebuild = self
            .wrap_cache
            .borrow()
            .as_ref()
            .is_none_or(|cache| cache.width != width);
        if needs_rebuild {
            *self.wrap_cache.borrow_mut() = Some(WrapCache {
                width,
                lines: wrap_source(&self.text, width),
            });
        }
        self.wrap_cache
            .borrow()
            .as_ref()
            .map(|cache| cache.lines.clone())
            .unwrap_or_default()
    }

    fn after_edit(&mut self) {
        self.preferred_column = None;
        self.wrap_cache.borrow_mut().take();
    }
}

impl Deref for ComposerBuffer {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<String> for ComposerBuffer {
    fn from(text: String) -> Self {
        let mut buffer = Self::default();
        buffer.set_text(text);
        buffer
    }
}

impl From<&str> for ComposerBuffer {
    fn from(text: &str) -> Self {
        text.to_owned().into()
    }
}

impl PartialEq<&str> for ComposerBuffer {
    fn eq(&self, other: &&str) -> bool {
        self.text == *other
    }
}

fn display_slice(source: &str) -> String {
    source.replace('\t', &" ".repeat(TAB_WIDTH))
}

fn grapheme_width(grapheme: &str) -> usize {
    if grapheme == "\t" {
        TAB_WIDTH
    } else {
        UnicodeWidthStr::width(grapheme)
    }
}

fn slice_width(source: &str) -> usize {
    source.graphemes(true).map(grapheme_width).sum()
}

fn trim_breakable_end(text: &str, start: usize, end: usize) -> usize {
    text[start..end]
        .grapheme_indices(true)
        .rev()
        .find_map(|(offset, grapheme)| {
            (!grapheme.chars().all(char::is_whitespace)).then_some(start + offset + grapheme.len())
        })
        .unwrap_or(start)
}

fn wrap_source(text: &str, width: u16) -> Vec<VisualLine> {
    let width = usize::from(width.max(1));
    let logical_lines = text.split('\n').collect::<Vec<_>>();
    let mut lines = Vec::new();
    let mut logical_start = 0;

    for (logical_index, logical) in logical_lines.iter().enumerate() {
        let logical_end = logical_start + logical.len();
        wrap_logical_line(text, logical_start, logical_end, width, &mut lines);

        let is_final = logical_index + 1 == logical_lines.len();
        if is_final
            && !text.ends_with('\n')
            && lines.last().is_some_and(|line| {
                line.source_end == logical_end
                    && slice_width(&text[line.source_start..line.display_end]) >= width
            })
        {
            lines.push(VisualLine {
                source_start: logical_end,
                source_end: logical_end,
                display_end: logical_end,
            });
        }
        logical_start = logical_end.saturating_add(1);
    }

    if lines.is_empty() {
        lines.push(VisualLine {
            source_start: 0,
            source_end: 0,
            display_end: 0,
        });
    }
    lines
}

fn wrap_logical_line(
    text: &str,
    start: usize,
    end: usize,
    width: usize,
    lines: &mut Vec<VisualLine>,
) {
    if start == end {
        lines.push(VisualLine {
            source_start: start,
            source_end: end,
            display_end: end,
        });
        return;
    }

    let graphemes = text[start..end]
        .grapheme_indices(true)
        .map(|(offset, grapheme)| SourceGrapheme {
            start: start + offset,
            end: start + offset + grapheme.len(),
            width: grapheme_width(grapheme),
            whitespace: grapheme.chars().all(char::is_whitespace),
        })
        .collect::<Vec<_>>();

    let mut line_start_index = 0;
    while line_start_index < graphemes.len() {
        let source_start = graphemes[line_start_index].start;
        let mut used = 0;
        let mut index = line_start_index;
        let mut last_break: Option<usize> = None;
        let mut has_text = false;
        let mut emitted = false;

        while index < graphemes.len() {
            let grapheme = graphemes[index];
            if used + grapheme.width > width && index > line_start_index {
                if grapheme.whitespace && has_text {
                    lines.push(VisualLine {
                        source_start,
                        source_end: grapheme.end,
                        display_end: trim_breakable_end(text, source_start, grapheme.start),
                    });
                    line_start_index = index + 1;
                } else if let Some(break_index) = last_break {
                    let source_end = graphemes[break_index - 1].end;
                    lines.push(VisualLine {
                        source_start,
                        source_end,
                        display_end: trim_breakable_end(text, source_start, source_end),
                    });
                    line_start_index = break_index;
                } else {
                    let source_end = graphemes[index - 1].end;
                    lines.push(VisualLine {
                        source_start,
                        source_end,
                        display_end: source_end,
                    });
                    line_start_index = index;
                }
                emitted = true;
                break;
            }

            used += grapheme.width;
            index += 1;
            if grapheme.whitespace {
                if has_text {
                    last_break = Some(index);
                }
            } else {
                has_text = true;
            }
        }

        if !emitted {
            lines.push(VisualLine {
                source_start,
                source_end: end,
                display_end: end,
            });
            break;
        }
    }
}

fn cursor_position(text: &str, lines: &[VisualLine], width: u16, cursor: usize) -> (usize, usize) {
    let row = lines
        .partition_point(|line| line.source_start <= cursor)
        .saturating_sub(1)
        .min(lines.len().saturating_sub(1));
    let line = &lines[row];
    let visible_cursor = cursor.min(line.display_end).max(line.source_start);
    let column = slice_width(&text[line.source_start..visible_cursor])
        .min(usize::from(width).saturating_sub(1));
    (row, column)
}

fn byte_at_column(text: &str, line: &VisualLine, target: usize) -> usize {
    let mut column = 0;
    for (offset, grapheme) in text[line.source_start..line.display_end].grapheme_indices(true) {
        let next = column + grapheme_width(grapheme);
        if next > target {
            return line.source_start + offset;
        }
        column = next;
    }
    line.display_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_then_hard_wraps_long_tokens() {
        let prompt = ComposerBuffer::from("the quick brown /a/very/long/path");
        assert_eq!(
            prompt.view(9, 20).lines,
            vec!["the quick", "brown", "/a/very/l", "ong/path"]
        );
    }

    #[test]
    fn preserves_explicit_lines_and_multiline_insertions() {
        let mut prompt = ComposerBuffer::from("first\nthird");
        prompt.move_to_line_start();
        prompt.insert_str("second\n");
        assert_eq!(prompt.as_str(), "first\nsecond\nthird");
        assert_eq!(prompt.view(20, 10).lines, vec!["first", "second", "third"]);
    }

    #[test]
    fn edits_at_grapheme_boundaries() {
        let mut prompt = ComposerBuffer::from("a👨‍👩‍👧‍👦b");
        assert!(prompt.move_left());
        assert!(prompt.backspace());
        assert_eq!(prompt.as_str(), "ab");
        assert_eq!(prompt.cursor(), 1);
        prompt.insert_char('界');
        assert_eq!(prompt.as_str(), "a界b");
        assert!(prompt.delete_forward());
        assert_eq!(prompt.as_str(), "a界");
    }

    #[test]
    fn vertical_motion_tracks_a_preferred_display_column() {
        let mut prompt = ComposerBuffer::from("12345\n12\n12345");
        assert!(prompt.move_up(20));
        assert_eq!(prompt.cursor(), 8);
        assert!(prompt.move_up(20));
        assert_eq!(prompt.cursor(), 5);
        assert!(prompt.move_down(20));
        assert_eq!(prompt.cursor(), 8);
        assert!(prompt.move_down(20));
        assert_eq!(prompt.cursor(), 14);
    }

    #[test]
    fn viewport_scroll_follows_the_cursor() {
        let mut prompt = ComposerBuffer::from("one\ntwo\nthree\nfour");
        let bottom = prompt.view(20, 2);
        assert_eq!(bottom.lines, vec!["three", "four"]);
        assert_eq!(bottom.scroll, 2);
        assert_eq!(bottom.cursor_row, 1);

        assert!(prompt.move_up(20));
        assert!(prompt.move_up(20));
        let top = prompt.view(20, 2);
        assert_eq!(top.lines, vec!["two", "three"]);
        assert_eq!(top.cursor_row, 0);
    }

    #[test]
    fn resize_reflows_and_keeps_wide_text_cursor_visible() {
        let prompt = ComposerBuffer::from("hello 👋 world");
        assert_eq!(prompt.desired_height(20), 1);
        assert_eq!(prompt.view(7, 8).lines, vec!["hello", "👋", "world"]);
        let narrow = prompt.view(7, 2);
        assert!(narrow.cursor_row < 2);
        assert!(narrow.cursor_column < 7);
    }

    #[test]
    fn exact_full_row_reserves_a_visible_insertion_row() {
        let prompt = ComposerBuffer::from("abcdefgh");
        assert_eq!(prompt.view(4, 10).lines, vec!["abcd", "efgh", ""]);
        assert_eq!(prompt.desired_height(4), 3);
    }

    #[test]
    fn cjk_combining_marks_spaces_and_tabs_keep_terminal_width_semantics() {
        let cjk = ComposerBuffer::from("界界界");
        assert_eq!(cjk.view(4, 10).lines, vec!["界界", "界"]);

        let combining = ComposerBuffer::from("e\u{301}e\u{301}e\u{301}");
        assert_eq!(combining.desired_height(2), 2);

        let spaces = ComposerBuffer::from("one   two");
        assert_eq!(spaces.view(6, 10).lines, vec!["one", "two"]);

        let tab = ComposerBuffer::from("\tx");
        assert_eq!(tab.view(5, 10).lines, vec!["    x", ""]);
    }

    #[test]
    fn every_grapheme_cursor_position_stays_inside_the_viewport() {
        let mut prompt = ComposerBuffer::from("👨‍👩‍👧‍👦 界 e\u{301}\n/a/long/unbroken/path");
        for width in 1..=12 {
            loop {
                let view = prompt.view(width, 3);
                assert!(view.cursor_row < 3);
                assert!(view.cursor_column < width);
                assert!(view.scroll < view.total_lines);
                if !prompt.move_left() {
                    break;
                }
            }
            prompt.move_to_document_end();
        }
    }
}
