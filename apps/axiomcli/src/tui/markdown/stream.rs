use std::ops::Range;

use super::{
    model::{MarkdownStyles, RenderedDocument},
    parse,
    render::{self, append_document},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RenderKey {
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
}

/// Retains completed top-level blocks while an assistant response streams.
///
/// The mutable tail is still parsed from source, which keeps this early
/// implementation straightforward and correct. Previous messages become a
/// cache hit, and long responses only re-render the newest Markdown block.
#[derive(Clone, Debug, Default)]
pub(in crate::tui) struct StreamingMarkdownRenderer {
    key: Option<RenderKey>,
    seen_source: String,
    frozen_source_bytes: usize,
    frozen: RenderedDocument,
    current: RenderedDocument,
}

impl StreamingMarkdownRenderer {
    pub(in crate::tui) fn render(
        &mut self,
        source: &str,
        width: usize,
        styles: MarkdownStyles,
        ascii: bool,
    ) -> &RenderedDocument {
        let key = RenderKey {
            width: width.max(4),
            styles,
            ascii,
        };
        let append_only = source.starts_with(&self.seen_source);
        if self.key != Some(key) || !append_only {
            self.reset(key);
        } else if source == self.seen_source {
            return &self.current;
        }
        self.seen_source.clear();
        self.seen_source.push_str(source);

        self.advance_checkpoint(source, key);
        self.current = self.frozen.clone();
        let tail = &source[self.frozen_source_bytes..];
        if !tail.is_empty() {
            let mut rendered_tail = render_source(tail, key);
            shift_source_offsets(&mut rendered_tail, self.frozen_source_bytes);
            let needs_gap = !self.current.lines.is_empty();
            append_document(&mut self.current, rendered_tail, needs_gap);
        }
        self.current.stable_source_prefix = self.frozen_source_bytes;
        &self.current
    }

    fn reset(&mut self, key: RenderKey) {
        self.key = Some(key);
        self.seen_source.clear();
        self.frozen_source_bytes = 0;
        self.frozen = RenderedDocument::default();
        self.current = RenderedDocument::default();
    }

    fn advance_checkpoint(&mut self, source: &str, key: RenderKey) {
        loop {
            let tail = &source[self.frozen_source_bytes..];
            if tail.is_empty() {
                return;
            }
            let preview = render_source(tail, key);
            let checkpoint = preview.stable_source_prefix.min(tail.len());
            if checkpoint == 0 || !tail.is_char_boundary(checkpoint) {
                return;
            }
            let mut stable = if checkpoint == tail.len() {
                preview
            } else {
                render_source(&tail[..checkpoint], key)
            };
            shift_source_offsets(&mut stable, self.frozen_source_bytes);
            let needs_gap = !self.frozen.lines.is_empty();
            append_document(&mut self.frozen, stable, needs_gap);
            self.frozen_source_bytes += checkpoint;
        }
    }
}

fn render_source(source: &str, key: RenderKey) -> RenderedDocument {
    let document = parse::parse(source);
    render::render(source, &document, key.width, key.styles, key.ascii)
}

fn shift_source_offsets(output: &mut RenderedDocument, offset: usize) {
    for source in output.line_sources.iter_mut().flatten() {
        shift_range(source, offset);
    }
    for link in &mut output.hyperlinks {
        shift_range(&mut link.source, offset);
    }
    for block in &mut output.code_blocks {
        shift_range(&mut block.source, offset);
    }
    for block in &mut output.blocks {
        shift_range(&mut block.source, offset);
    }
}

fn shift_range(range: &mut Range<usize>, offset: usize) {
    range.start = range.start.saturating_add(offset);
    range.end = range.end.saturating_add(offset);
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Style};

    use super::*;

    fn styles() -> MarkdownStyles {
        MarkdownStyles {
            text: Style::default().fg(Color::White),
            muted: Style::default().fg(Color::DarkGray),
            accent: Style::default().fg(Color::Magenta),
            accent_bright: Style::default().fg(Color::LightMagenta),
            code: Style::default().fg(Color::Cyan),
            code_surface: Style::default().fg(Color::White).bg(Color::Black),
            border: Style::default().fg(Color::DarkGray),
            quote: Style::default().fg(Color::Magenta),
            link: Style::default().fg(Color::LightMagenta),
            warning: Style::default().fg(Color::Yellow),
            success: Style::default().fg(Color::Green),
            truecolor: false,
        }
    }

    fn plain(output: &RenderedDocument) -> Vec<String> {
        output
            .lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn freezes_completed_blocks_and_matches_a_one_shot_render() {
        let source = "# Result\n\nFirst paragraph.\n\n- one\n- two\n\nFinal **words**.";
        let mut streaming = StreamingMarkdownRenderer::default();
        for end in source
            .char_indices()
            .map(|(index, character)| index + character.len_utf8())
        {
            let _ = streaming.render(&source[..end], 32, styles(), false);
        }
        assert!(streaming.frozen_source_bytes > 0);
        let expected = render_source(
            source,
            RenderKey {
                width: 32,
                styles: styles(),
                ascii: false,
            },
        );
        assert_eq!(plain(&streaming.current), plain(&expected));
    }

    #[test]
    fn non_append_edits_and_width_changes_reset_safely() {
        let mut streaming = StreamingMarkdownRenderer::default();
        let _ = streaming.render("one\n\ntwo", 30, styles(), false);
        let edited = streaming.render("replacement", 12, styles(), false);
        assert_eq!(plain(edited), ["replacement"]);
        assert_eq!(streaming.frozen_source_bytes, 0);
    }
}
