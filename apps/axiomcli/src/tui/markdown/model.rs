use std::ops::Range;

use ratatui::{style::Style, text::Line};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TableAlignment {
    None,
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CalloutKind {
    Quote,
    Note,
    Tip,
    Important,
    Warning,
    Caution,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ElementKind {
    Paragraph,
    Heading(u8),
    BlockQuote(CalloutKind),
    CodeBlock { info: String },
    HtmlBlock,
    List { start: Option<u64> },
    Item,
    FootnoteDefinition(String),
    DefinitionList,
    DefinitionTitle,
    DefinitionValue,
    Table { alignments: Vec<TableAlignment> },
    TableHead,
    TableRow,
    TableCell,
    Emphasis,
    Strong,
    Strikethrough,
    Superscript,
    Subscript,
    Link { destination: String },
    Image { destination: String },
    MetadataBlock,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LeafKind {
    Text(String),
    Code(String),
    InlineMath(String),
    DisplayMath(String),
    Html(String),
    FootnoteReference(String),
    SoftBreak,
    HardBreak,
    Rule,
    TaskListMarker(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NodeKind {
    Element {
        element: ElementKind,
        children: Vec<Node>,
    },
    Leaf(LeafKind),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Node {
    pub(super) kind: NodeKind,
    pub(super) source: Range<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Document {
    pub(super) nodes: Vec<Node>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) struct MarkdownStyles {
    pub(in crate::tui) text: Style,
    pub(in crate::tui) muted: Style,
    pub(in crate::tui) accent: Style,
    pub(in crate::tui) accent_bright: Style,
    pub(in crate::tui) code: Style,
    pub(in crate::tui) code_surface: Style,
    pub(in crate::tui) border: Style,
    pub(in crate::tui) quote: Style,
    pub(in crate::tui) link: Style,
    pub(in crate::tui) warning: Style,
    pub(in crate::tui) success: Style,
    pub(in crate::tui) truecolor: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HyperlinkRegion {
    pub(super) line_index: usize,
    pub(super) columns: Range<usize>,
    pub(super) destination: String,
    pub(super) source: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CodeBlockRegion {
    pub(super) info: String,
    pub(super) body: String,
    pub(super) output_lines: Range<usize>,
    pub(super) source: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RenderedBlockKind {
    Paragraph,
    Heading,
    Quote,
    Code,
    List,
    Table,
    Rule,
    Html,
    Math,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct RenderedBlockRegion {
    pub(super) kind: RenderedBlockKind,
    pub(super) output_lines: Range<usize>,
    pub(super) source: Range<usize>,
}

/// Render-ready terminal lines plus enough structural information for future
/// selection, copy, link activation, fenced-block extensions, and ACP clients.
#[derive(Clone, Debug, Default)]
pub(in crate::tui) struct RenderedDocument {
    pub(in crate::tui) lines: Vec<Line<'static>>,
    pub(super) line_sources: Vec<Option<Range<usize>>>,
    pub(super) hyperlinks: Vec<HyperlinkRegion>,
    pub(super) code_blocks: Vec<CodeBlockRegion>,
    pub(super) blocks: Vec<RenderedBlockRegion>,
    /// Everything before this source byte is a completed top-level block and
    /// can be cached by a future retained streaming view.
    pub(super) stable_source_prefix: usize,
}

impl RenderedDocument {
    pub(super) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}
