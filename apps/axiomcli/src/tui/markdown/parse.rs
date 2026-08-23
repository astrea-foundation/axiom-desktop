use std::ops::Range;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd,
};

use super::model::{CalloutKind, Document, ElementKind, LeafKind, Node, NodeKind, TableAlignment};

struct Frame {
    element: Option<ElementKind>,
    source_start: usize,
    children: Vec<Node>,
}

pub(super) fn parse(source: &str) -> Document {
    let options = Options::ENABLE_GFM
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH;
    let mut frames = vec![Frame {
        element: None,
        source_start: 0,
        children: Vec::new(),
    }];

    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        // `pulldown-cmark` intentionally accepts both `~single~` and
        // `~~double~~` as strikethrough. Model prose frequently uses a single
        // tilde for approximation, so preserve that form literally.
        if matches!(&event, Event::Start(Tag::Strikethrough)) && !is_double_tilde(source, &range) {
            push_leaf(
                &mut frames,
                LeafKind::Text("~".to_owned()),
                range.start..range.start.saturating_add(1),
            );
            continue;
        }
        if matches!(&event, Event::End(TagEnd::Strikethrough)) && !is_double_tilde(source, &range) {
            push_leaf(
                &mut frames,
                LeafKind::Text("~".to_owned()),
                range.end.saturating_sub(1)..range.end,
            );
            continue;
        }
        match event {
            Event::Start(tag) => frames.push(Frame {
                element: Some(element_from_tag(tag)),
                source_start: range.start,
                children: Vec::new(),
            }),
            Event::End(_end) => close_frame(&mut frames, range),
            Event::Text(text) => push_leaf(&mut frames, LeafKind::Text(text.into_string()), range),
            Event::Code(text) => push_leaf(&mut frames, LeafKind::Code(text.into_string()), range),
            Event::InlineMath(text) => {
                push_leaf(&mut frames, LeafKind::InlineMath(text.into_string()), range);
            }
            Event::DisplayMath(text) => {
                push_leaf(
                    &mut frames,
                    LeafKind::DisplayMath(text.into_string()),
                    range,
                );
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                push_leaf(&mut frames, LeafKind::Html(text.into_string()), range);
            }
            Event::FootnoteReference(label) => push_leaf(
                &mut frames,
                LeafKind::FootnoteReference(label.into_string()),
                range,
            ),
            Event::SoftBreak => push_leaf(&mut frames, LeafKind::SoftBreak, range),
            Event::HardBreak => push_leaf(&mut frames, LeafKind::HardBreak, range),
            Event::Rule => push_leaf(&mut frames, LeafKind::Rule, range),
            Event::TaskListMarker(checked) => {
                push_leaf(&mut frames, LeafKind::TaskListMarker(checked), range);
            }
        }
    }

    while frames.len() > 1 {
        close_frame(&mut frames, source.len()..source.len());
    }
    Document {
        nodes: frames.pop().map_or_else(Vec::new, |frame| frame.children),
    }
}

fn is_double_tilde(source: &str, range: &Range<usize>) -> bool {
    source
        .get(range.start..)
        .is_some_and(|remainder| remainder.starts_with("~~"))
}

fn close_frame(frames: &mut Vec<Frame>, end_range: Range<usize>) {
    let Some(frame) = frames.pop() else {
        return;
    };
    let Some(element) = frame.element else {
        frames.push(frame);
        return;
    };
    let node = Node {
        kind: NodeKind::Element {
            element,
            children: frame.children,
        },
        source: frame.source_start..end_range.end.max(frame.source_start),
    };
    if let Some(parent) = frames.last_mut() {
        parent.children.push(node);
    }
}

fn push_leaf(frames: &mut [Frame], leaf: LeafKind, source: Range<usize>) {
    if let Some(frame) = frames.last_mut() {
        frame.children.push(Node {
            kind: NodeKind::Leaf(leaf),
            source,
        });
    }
}

fn element_from_tag(tag: Tag<'_>) -> ElementKind {
    match tag {
        Tag::Paragraph => ElementKind::Paragraph,
        Tag::Heading { level, .. } => ElementKind::Heading(match level {
            HeadingLevel::H1 => 1,
            HeadingLevel::H2 => 2,
            HeadingLevel::H3 => 3,
            HeadingLevel::H4 => 4,
            HeadingLevel::H5 => 5,
            HeadingLevel::H6 => 6,
        }),
        Tag::BlockQuote(kind) => ElementKind::BlockQuote(match kind {
            None => CalloutKind::Quote,
            Some(BlockQuoteKind::Note) => CalloutKind::Note,
            Some(BlockQuoteKind::Tip) => CalloutKind::Tip,
            Some(BlockQuoteKind::Important) => CalloutKind::Important,
            Some(BlockQuoteKind::Warning) => CalloutKind::Warning,
            Some(BlockQuoteKind::Caution) => CalloutKind::Caution,
        }),
        Tag::CodeBlock(kind) => ElementKind::CodeBlock {
            info: match kind {
                CodeBlockKind::Indented => String::new(),
                CodeBlockKind::Fenced(info) => info.into_string(),
            },
        },
        Tag::HtmlBlock => ElementKind::HtmlBlock,
        Tag::List(start) => ElementKind::List { start },
        Tag::Item => ElementKind::Item,
        Tag::FootnoteDefinition(label) => ElementKind::FootnoteDefinition(label.into_string()),
        Tag::DefinitionList => ElementKind::DefinitionList,
        Tag::DefinitionListTitle => ElementKind::DefinitionTitle,
        Tag::DefinitionListDefinition => ElementKind::DefinitionValue,
        Tag::Table(alignments) => ElementKind::Table {
            alignments: alignments.into_iter().map(table_alignment).collect(),
        },
        Tag::TableHead => ElementKind::TableHead,
        Tag::TableRow => ElementKind::TableRow,
        Tag::TableCell => ElementKind::TableCell,
        Tag::Emphasis => ElementKind::Emphasis,
        Tag::Strong => ElementKind::Strong,
        Tag::Strikethrough => ElementKind::Strikethrough,
        Tag::Superscript => ElementKind::Superscript,
        Tag::Subscript => ElementKind::Subscript,
        Tag::Link { dest_url, .. } => ElementKind::Link {
            destination: dest_url.into_string(),
        },
        Tag::Image { dest_url, .. } => ElementKind::Image {
            destination: dest_url.into_string(),
        },
        Tag::MetadataBlock(_) => ElementKind::MetadataBlock,
    }
}

const fn table_alignment(alignment: Alignment) -> TableAlignment {
    match alignment {
        Alignment::None => TableAlignment::None,
        Alignment::Left => TableAlignment::Left,
        Alignment::Center => TableAlignment::Center,
        Alignment::Right => TableAlignment::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_nested_structure_and_source_ranges() {
        let source = "# Head\n\n- **bold** and [link](https://example.com)\n";
        let document = parse(source);
        assert_eq!(document.nodes.len(), 2);
        assert_eq!(document.nodes[0].source, 0..7);
        assert!(matches!(
            document.nodes[1].kind,
            NodeKind::Element {
                element: ElementKind::List { .. },
                ..
            }
        ));
    }

    #[test]
    fn enables_gfm_tasks_tables_and_math() {
        let document = parse("- [x] done\n\n| a | b |\n|---|---|\n| $x$ | y |\n");
        let debug = format!("{document:?}");
        assert!(debug.contains("TaskListMarker(true)"));
        assert!(debug.contains("Table"));
        assert!(debug.contains("InlineMath"));
    }

    #[test]
    fn only_double_tildes_create_strikethrough() {
        let document = parse("about ~10% but ~~removed~~");
        let debug = format!("{document:?}");
        assert_eq!(debug.matches("Strikethrough").count(), 1);
        assert!(debug.contains("Text(\"~\")"));
    }
}
