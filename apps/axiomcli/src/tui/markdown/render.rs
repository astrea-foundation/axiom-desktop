use std::{cmp, fmt::Write as _, ops::Range};

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr as _;

use super::{
    highlight,
    model::{
        CalloutKind, CodeBlockRegion, Document, ElementKind, HyperlinkRegion, LeafKind,
        MarkdownStyles, Node, NodeKind, RenderedBlockKind, RenderedBlockRegion, RenderedDocument,
        TableAlignment,
    },
};

#[derive(Clone, Debug)]
struct InlineRun {
    text: String,
    style: Style,
    source: Range<usize>,
    link: Option<String>,
    hard_break: bool,
}

#[derive(Clone, Debug)]
struct Atom {
    text: String,
    width: usize,
    style: Style,
    source: Range<usize>,
    link: Option<String>,
}

#[derive(Clone, Debug)]
enum WrapToken {
    Word(Vec<Atom>),
    Space,
    Break,
}

#[derive(Clone, Debug)]
struct InlineContext {
    style: Style,
    link: Option<String>,
    image: bool,
}

#[derive(Clone, Debug)]
struct TableCell {
    runs: Vec<InlineRun>,
    source: Range<usize>,
}

#[derive(Clone, Debug)]
struct TableRow {
    cells: Vec<TableCell>,
    source: Range<usize>,
    header: bool,
}

pub(super) fn render(
    source: &str,
    document: &Document,
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
) -> RenderedDocument {
    let width = width.max(4);
    let mut output = render_sequence(&document.nodes, width, styles, ascii, false);
    output.stable_source_prefix = stable_prefix(source, document);
    output
}

fn render_sequence(
    nodes: &[Node],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
    compact: bool,
) -> RenderedDocument {
    let mut output = RenderedDocument::default();
    for node in nodes {
        let rendered = render_node(node, width, styles, ascii);
        if rendered.is_empty() {
            continue;
        }
        let blank_before = !compact && !output.is_empty();
        append_document(&mut output, rendered, blank_before);
    }
    output
}

fn render_node(node: &Node, width: usize, styles: MarkdownStyles, ascii: bool) -> RenderedDocument {
    match &node.kind {
        NodeKind::Leaf(LeafKind::Rule) => render_rule(node, width, styles, ascii),
        NodeKind::Leaf(leaf) => {
            let runs = collect_leaf_as_runs(leaf, node.source.clone(), styles);
            let mut output = wrap_runs(&runs, width, styles.text);
            record_block(&mut output, RenderedBlockKind::Other, node.source.clone());
            output
        }
        NodeKind::Element { element, children } => match element {
            ElementKind::Paragraph => render_paragraph(node, children, width, styles),
            ElementKind::Heading(level) => render_heading(node, children, width, styles, *level),
            ElementKind::BlockQuote(kind) => {
                render_quote(node, children, width, styles, ascii, *kind)
            }
            ElementKind::CodeBlock { info } => {
                render_code_block(node, children, width, styles, ascii, info)
            }
            ElementKind::HtmlBlock | ElementKind::MetadataBlock => {
                render_html(node, children, width, styles)
            }
            ElementKind::List { start } => {
                render_list(node, children, width, styles, ascii, *start)
            }
            ElementKind::Table { alignments } => {
                render_table(node, children, width, styles, ascii, alignments)
            }
            ElementKind::FootnoteDefinition(label) => {
                render_footnote(node, children, width, styles, ascii, label)
            }
            ElementKind::DefinitionList
            | ElementKind::DefinitionTitle
            | ElementKind::DefinitionValue
            | ElementKind::Item
            | ElementKind::TableHead
            | ElementKind::TableRow
            | ElementKind::TableCell => {
                let mut output = render_sequence(children, width, styles, ascii, true);
                record_block(&mut output, RenderedBlockKind::Other, node.source.clone());
                output
            }
            ElementKind::Emphasis
            | ElementKind::Strong
            | ElementKind::Strikethrough
            | ElementKind::Superscript
            | ElementKind::Subscript
            | ElementKind::Link { .. }
            | ElementKind::Image { .. } => {
                let runs = collect_inlines(children, styles, styles.text);
                let mut output = wrap_runs(&runs, width, styles.text);
                record_block(&mut output, RenderedBlockKind::Other, node.source.clone());
                output
            }
        },
    }
}

fn render_paragraph(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
) -> RenderedDocument {
    let runs = collect_inlines(children, styles, styles.text);
    let mut output = wrap_runs(&runs, width, styles.text);
    record_block(
        &mut output,
        if children
            .iter()
            .any(|child| matches!(child.kind, NodeKind::Leaf(LeafKind::DisplayMath(_))))
        {
            RenderedBlockKind::Math
        } else {
            RenderedBlockKind::Paragraph
        },
        node.source.clone(),
    );
    output
}

fn render_heading(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
    level: u8,
) -> RenderedDocument {
    let style = match level {
        1 => styles
            .accent_bright
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => styles.accent.add_modifier(Modifier::BOLD),
        3 => styles
            .accent
            .add_modifier(Modifier::BOLD | Modifier::ITALIC),
        _ => styles.text.add_modifier(Modifier::BOLD),
    };
    let runs = collect_inlines(children, styles, style);
    let mut output = wrap_runs(&runs, width, style);
    record_block(&mut output, RenderedBlockKind::Heading, node.source.clone());
    output
}

fn render_quote(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
    kind: CalloutKind,
) -> RenderedDocument {
    let rail_width = 2;
    let mut inner = render_sequence(
        children,
        width.saturating_sub(rail_width).max(2),
        styles,
        ascii,
        true,
    );
    if kind != CalloutKind::Quote {
        let (label, style) = match kind {
            CalloutKind::Note => ("NOTE", styles.accent),
            CalloutKind::Tip => ("TIP", styles.success),
            CalloutKind::Important => ("IMPORTANT", styles.accent_bright),
            CalloutKind::Warning | CalloutKind::Caution => ("WARNING", styles.warning),
            CalloutKind::Quote => unreachable!(),
        };
        inner.lines.insert(
            0,
            Line::from(Span::styled(label, style.add_modifier(Modifier::BOLD))),
        );
        inner.line_sources.insert(0, Some(node.source.clone()));
        shift_metadata_lines(&mut inner, 1);
    }
    if inner.is_empty() {
        inner.lines.push(Line::default());
        inner.line_sources.push(Some(node.source.clone()));
    }
    prepend_lines(
        &mut inner,
        &[Span::styled(if ascii { "| " } else { "│ " }, styles.quote)],
        rail_width,
    );
    record_block(&mut inner, RenderedBlockKind::Quote, node.source.clone());
    inner
}

fn render_list(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
    start: Option<u64>,
) -> RenderedDocument {
    let items = children
        .iter()
        .filter_map(|child| match &child.kind {
            NodeKind::Element {
                element: ElementKind::Item,
                children,
            } => Some((child, children.as_slice())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut output = RenderedDocument::default();
    for (index, (item, item_children)) in items.into_iter().enumerate() {
        let marker = start.map_or_else(
            || {
                if ascii {
                    "- ".to_owned()
                } else {
                    "• ".to_owned()
                }
            },
            |first| format!("{}. ", first.saturating_add(index as u64)),
        );
        let marker_width = display_width(&marker);
        let mut rendered = render_list_item(
            item_children,
            width.saturating_sub(marker_width).max(2),
            styles,
            ascii,
        );
        if rendered.is_empty() {
            rendered.lines.push(Line::default());
            rendered.line_sources.push(Some(item.source.clone()));
        }
        prepend_first_and_rest(
            &mut rendered,
            &[Span::styled(
                marker,
                styles.accent.add_modifier(Modifier::BOLD),
            )],
            &[Span::raw(" ".repeat(marker_width))],
            marker_width,
        );
        append_document(&mut output, rendered, false);
    }
    record_block(&mut output, RenderedBlockKind::List, node.source.clone());
    output
}

/// Tight `CommonMark` list items expose their inline events directly beneath the
/// item, and GFM task markers can precede an otherwise normal paragraph. Fold
/// that leading inline material into one flow so `• [✓] text` stays together.
fn render_list_item(
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
) -> RenderedDocument {
    let context = InlineContext {
        style: styles.text,
        link: None,
        image: false,
    };
    let mut runs = Vec::new();
    let mut consumed = 0;
    while let Some(node) = children.get(consumed) {
        match &node.kind {
            NodeKind::Leaf(LeafKind::Rule) => break,
            NodeKind::Leaf(_)
            | NodeKind::Element {
                element:
                    ElementKind::Emphasis
                    | ElementKind::Strong
                    | ElementKind::Strikethrough
                    | ElementKind::Superscript
                    | ElementKind::Subscript
                    | ElementKind::Link { .. }
                    | ElementKind::Image { .. },
                ..
            } => {
                collect_inline_node(node, styles, &context, &mut runs);
                consumed += 1;
            }
            NodeKind::Element {
                element: ElementKind::Paragraph,
                children: paragraph,
            } => {
                runs.extend(collect_inlines(paragraph, styles, styles.text));
                consumed += 1;
                break;
            }
            NodeKind::Element { .. } => break,
        }
    }

    let mut output = if runs.is_empty() {
        RenderedDocument::default()
    } else {
        wrap_runs(&runs, width, styles.text)
    };
    let rest = render_sequence(&children[consumed..], width, styles, ascii, true);
    append_document(&mut output, rest, false);
    output
}

fn render_code_block(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
    info: &str,
) -> RenderedDocument {
    let body = plain_text(children);
    if let Some(mut extension) = render_fenced_extension(info, &body, width, styles, ascii) {
        record_block(&mut extension, RenderedBlockKind::Code, node.source.clone());
        return extension;
    }

    let vertical = if ascii { "|" } else { "│" };
    let top_left = if ascii { "+" } else { "╭" };
    let bottom_left = if ascii { "+" } else { "╰" };
    let horizontal = if ascii { "-" } else { "─" };
    let body_width = width.saturating_sub(2).max(1);
    let language = info.split_whitespace().next().unwrap_or_default();
    let mut output = RenderedDocument::default();
    let header = if language.is_empty() {
        vec![
            Span::styled(top_left, styles.border),
            Span::styled(horizontal.repeat(width.saturating_sub(1)), styles.border),
        ]
    } else {
        let label = clip_plain(language, width.saturating_sub(4));
        let suffix_width = width.saturating_sub(display_width(&label) + 4);
        vec![
            Span::styled(top_left, styles.border),
            Span::styled(format!("{horizontal} "), styles.border),
            Span::styled(label, styles.muted.add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(" {}", horizontal.repeat(suffix_width)),
                styles.border,
            ),
        ]
    };
    push_line(&mut output, header, Some(node.source.clone()));
    let first_body_line = output.lines.len();
    let highlighted = highlight::code_lines(&body, language, styles.code_surface, styles.truecolor);
    for spans in highlighted {
        let mut clipped = clip_spans(&spans, body_width, styles.code_surface);
        let used = spans_width(&clipped);
        clipped.push(Span::styled(
            " ".repeat(body_width.saturating_sub(used)),
            styles.code_surface,
        ));
        let mut line = vec![
            Span::styled(vertical, styles.border),
            Span::styled(" ", styles.code_surface),
        ];
        line.extend(clipped);
        push_line(&mut output, line, body_source_range(children));
    }
    let body_end = output.lines.len();
    push_line(
        &mut output,
        vec![
            Span::styled(bottom_left, styles.border),
            Span::styled(horizontal.repeat(width.saturating_sub(1)), styles.border),
        ],
        Some(node.source.clone()),
    );
    output.code_blocks.push(CodeBlockRegion {
        info: info.to_owned(),
        body,
        output_lines: first_body_line..body_end,
        source: node.source.clone(),
    });
    record_block(&mut output, RenderedBlockKind::Code, node.source.clone());
    output
}

/// Kept as the single dispatch point for future rich fenced renderers. Mermaid,
/// patches, JSON trees, or ACP-provided artifacts can be added without changing
/// Markdown parsing or the ordinary code-block path.
fn render_fenced_extension(
    _info: &str,
    _body: &str,
    _width: usize,
    _styles: MarkdownStyles,
    _ascii: bool,
) -> Option<RenderedDocument> {
    None
}

fn render_table(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
    alignments: &[TableAlignment],
) -> RenderedDocument {
    let rows = table_rows(children, styles);
    let columns = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    if columns == 0 {
        return RenderedDocument::default();
    }
    let natural = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|row| row.cells.get(column))
                .map(|cell| inline_plain_width(&cell.runs).clamp(3, 40))
                .max()
                .unwrap_or(3)
        })
        .collect::<Vec<_>>();
    let border_cost = columns.saturating_mul(3).saturating_add(1);
    let available = width.saturating_sub(border_cost);
    let mut output = if available < columns.saturating_mul(3) {
        render_table_records(&rows, width, styles, ascii)
    } else {
        let widths = fit_columns(natural, available);
        render_table_grid(&rows, &widths, alignments, styles, ascii)
    };
    record_block(&mut output, RenderedBlockKind::Table, node.source.clone());
    output
}

fn render_table_grid(
    rows: &[TableRow],
    widths: &[usize],
    alignments: &[TableAlignment],
    styles: MarkdownStyles,
    ascii: bool,
) -> RenderedDocument {
    let mut output = RenderedDocument::default();
    push_line(
        &mut output,
        vec![Span::styled(
            table_border(widths, ascii, BorderRow::Top),
            styles.border,
        )],
        None,
    );
    for (row_index, row) in rows.iter().enumerate() {
        append_table_row(&mut output, row, widths, alignments, styles, ascii);
        if row.header && row_index + 1 < rows.len() {
            push_line(
                &mut output,
                vec![Span::styled(
                    table_border(widths, ascii, BorderRow::Middle),
                    styles.border,
                )],
                None,
            );
        }
    }
    push_line(
        &mut output,
        vec![Span::styled(
            table_border(widths, ascii, BorderRow::Bottom),
            styles.border,
        )],
        None,
    );
    output
}

fn append_table_row(
    output: &mut RenderedDocument,
    row: &TableRow,
    widths: &[usize],
    alignments: &[TableAlignment],
    styles: MarkdownStyles,
    ascii: bool,
) {
    let cells = widths
        .iter()
        .enumerate()
        .map(|(column, width)| {
            row.cells.get(column).map_or_else(
                || {
                    let mut blank = RenderedDocument::default();
                    push_line(&mut blank, vec![Span::raw("")], Some(row.source.clone()));
                    blank
                },
                |cell| {
                    let mut rendered = wrap_runs(&cell.runs, *width, styles.text);
                    for source in &mut rendered.line_sources {
                        if source.is_none() {
                            *source = Some(cell.source.clone());
                        }
                    }
                    rendered
                },
            )
        })
        .collect::<Vec<_>>();
    let height = cells.iter().map(|cell| cell.lines.len()).max().unwrap_or(1);
    let vertical = if ascii { "|" } else { "│" };
    for line_index in 0..height {
        let output_line = output.lines.len();
        let mut spans = vec![Span::styled(vertical, styles.border), Span::raw(" ")];
        let mut column_offset = 2;
        for (column, cell) in cells.iter().enumerate() {
            let source_line = cell.lines.get(line_index);
            let cell_width = source_line.map_or(0, Line::width);
            let alignment = alignments
                .get(column)
                .copied()
                .unwrap_or(TableAlignment::None);
            let remaining = widths[column].saturating_sub(cell_width);
            let left_padding = match alignment {
                TableAlignment::Right => remaining,
                TableAlignment::Center => remaining / 2,
                TableAlignment::None | TableAlignment::Left => 0,
            };
            let right_padding = remaining.saturating_sub(left_padding);
            spans.push(Span::raw(" ".repeat(left_padding)));
            if let Some(line) = source_line {
                spans.extend(line.spans.clone());
            }
            spans.push(Span::raw(" ".repeat(right_padding)));
            for link in cell
                .hyperlinks
                .iter()
                .filter(|link| link.line_index == line_index)
            {
                output.hyperlinks.push(HyperlinkRegion {
                    line_index: output_line,
                    columns: (link.columns.start + column_offset + left_padding)
                        ..(link.columns.end + column_offset + left_padding),
                    destination: link.destination.clone(),
                    source: link.source.clone(),
                });
            }
            spans.push(Span::raw(" "));
            spans.push(Span::styled(vertical, styles.border));
            if column + 1 < widths.len() {
                spans.push(Span::raw(" "));
            }
            column_offset += widths[column] + 3;
        }
        push_line(output, spans, Some(row.source.clone()));
    }
}

fn render_table_records(
    rows: &[TableRow],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
) -> RenderedDocument {
    let Some(header) = rows.iter().find(|row| row.header) else {
        return RenderedDocument::default();
    };
    let data = rows.iter().filter(|row| !row.header).collect::<Vec<_>>();
    let mut output = RenderedDocument::default();
    for (row_index, row) in data.iter().enumerate() {
        if row_index > 0 {
            push_line(&mut output, Vec::new(), None);
        }
        push_line(
            &mut output,
            vec![Span::styled(
                format!("{} {}", if ascii { "row" } else { "◇" }, row_index + 1),
                styles.muted.add_modifier(Modifier::BOLD),
            )],
            Some(row.source.clone()),
        );
        for (column, value) in row.cells.iter().enumerate() {
            let label = header
                .cells
                .get(column)
                .map(|cell| inline_plain(&cell.runs))
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| format!("column {}", column + 1));
            let prefix = format!("  {label}: ");
            let prefix_width = display_width(&prefix);
            let mut rendered = wrap_runs(
                &value.runs,
                width.saturating_sub(prefix_width).max(2),
                styles.text,
            );
            prepend_first_and_rest(
                &mut rendered,
                &[Span::styled(
                    prefix,
                    styles.accent.add_modifier(Modifier::BOLD),
                )],
                &[Span::raw(" ".repeat(prefix_width))],
                prefix_width,
            );
            append_document(&mut output, rendered, false);
        }
    }
    output
}

fn render_rule(node: &Node, width: usize, styles: MarkdownStyles, ascii: bool) -> RenderedDocument {
    let mut output = RenderedDocument::default();
    push_line(
        &mut output,
        vec![Span::styled(
            if ascii {
                "-".repeat(width)
            } else {
                "─".repeat(width)
            },
            styles.border,
        )],
        Some(node.source.clone()),
    );
    record_block(&mut output, RenderedBlockKind::Rule, node.source.clone());
    output
}

fn render_html(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
) -> RenderedDocument {
    let text = plain_text(children);
    let runs = vec![InlineRun {
        text,
        style: styles.muted,
        source: node.source.clone(),
        link: None,
        hard_break: false,
    }];
    let mut output = wrap_runs(&runs, width, styles.muted);
    record_block(&mut output, RenderedBlockKind::Html, node.source.clone());
    output
}

fn render_footnote(
    node: &Node,
    children: &[Node],
    width: usize,
    styles: MarkdownStyles,
    ascii: bool,
    label: &str,
) -> RenderedDocument {
    let prefix = format!("{}[{label}] ", if ascii { "" } else { "↳ " });
    let prefix_width = display_width(&prefix);
    let mut output = render_sequence(
        children,
        width.saturating_sub(prefix_width).max(2),
        styles,
        ascii,
        true,
    );
    prepend_first_and_rest(
        &mut output,
        &[Span::styled(prefix, styles.muted)],
        &[Span::raw(" ".repeat(prefix_width))],
        prefix_width,
    );
    record_block(&mut output, RenderedBlockKind::Other, node.source.clone());
    output
}

fn collect_inlines(nodes: &[Node], styles: MarkdownStyles, base: Style) -> Vec<InlineRun> {
    let mut output = Vec::new();
    let context = InlineContext {
        style: base,
        link: None,
        image: false,
    };
    for node in nodes {
        collect_inline_node(node, styles, &context, &mut output);
    }
    output
}

fn collect_inline_node(
    node: &Node,
    styles: MarkdownStyles,
    context: &InlineContext,
    output: &mut Vec<InlineRun>,
) {
    match &node.kind {
        NodeKind::Leaf(leaf) => output.extend(collect_leaf_as_runs_with_context(
            leaf,
            node.source.clone(),
            styles,
            context,
        )),
        NodeKind::Element { element, children } => {
            let mut nested = context.clone();
            match element {
                ElementKind::Emphasis => {
                    nested.style = nested.style.add_modifier(Modifier::ITALIC);
                }
                ElementKind::Strong => {
                    nested.style = nested.style.add_modifier(Modifier::BOLD);
                }
                ElementKind::Strikethrough => {
                    nested.style = nested.style.add_modifier(Modifier::CROSSED_OUT);
                }
                ElementKind::Superscript => {
                    push_run(
                        output,
                        "^",
                        styles.muted,
                        node.source.clone(),
                        nested.link.clone(),
                    );
                }
                ElementKind::Subscript => {
                    push_run(
                        output,
                        "_",
                        styles.muted,
                        node.source.clone(),
                        nested.link.clone(),
                    );
                }
                ElementKind::Link { destination } => {
                    nested.link = safe_destination(destination);
                    nested.style = nested.style.patch(styles.link);
                }
                ElementKind::Image { destination } => {
                    nested.link = safe_destination(destination);
                    nested.image = true;
                    nested.style = nested.style.patch(styles.link);
                    push_run(
                        output,
                        "image: ",
                        styles.muted,
                        node.source.clone(),
                        nested.link.clone(),
                    );
                }
                _ => {}
            }
            for child in children {
                collect_inline_node(child, styles, &nested, output);
            }
        }
    }
}

fn collect_leaf_as_runs(
    leaf: &LeafKind,
    source: Range<usize>,
    styles: MarkdownStyles,
) -> Vec<InlineRun> {
    collect_leaf_as_runs_with_context(
        leaf,
        source,
        styles,
        &InlineContext {
            style: styles.text,
            link: None,
            image: false,
        },
    )
}

fn collect_leaf_as_runs_with_context(
    leaf: &LeafKind,
    source: Range<usize>,
    styles: MarkdownStyles,
    context: &InlineContext,
) -> Vec<InlineRun> {
    let (text, style, hard_break) = match leaf {
        LeafKind::Text(text) => (clean_text(text), context.style, false),
        LeafKind::Code(text) => (clean_text(text), styles.code, false),
        LeafKind::InlineMath(text) => (format!("${}$", clean_text(text)), styles.code, false),
        LeafKind::DisplayMath(text) => (format!("$${}$$", clean_text(text)), styles.code, false),
        LeafKind::Html(text) => (clean_text(text), styles.muted, false),
        LeafKind::FootnoteReference(label) => (format!("[^{label}]"), styles.accent, false),
        LeafKind::SoftBreak => (" ".to_owned(), context.style, false),
        LeafKind::HardBreak => (String::new(), context.style, true),
        LeafKind::Rule => (String::new(), styles.border, false),
        LeafKind::TaskListMarker(checked) => (
            if *checked { "[✓] " } else { "[ ] " }.to_owned(),
            if *checked {
                styles.success
            } else {
                styles.muted
            },
            false,
        ),
    };
    vec![InlineRun {
        text,
        style,
        source,
        link: context.link.clone(),
        hard_break,
    }]
}

fn push_run(
    output: &mut Vec<InlineRun>,
    text: &str,
    style: Style,
    source: Range<usize>,
    link: Option<String>,
) {
    output.push(InlineRun {
        text: text.to_owned(),
        style,
        source,
        link,
        hard_break: false,
    });
}

fn wrap_runs(runs: &[InlineRun], width: usize, space_style: Style) -> RenderedDocument {
    let width = width.max(1);
    let tokens = wrap_tokens(runs);
    let mut atom_lines: Vec<Vec<Atom>> = Vec::new();
    let mut current = Vec::new();
    let mut current_width = 0;
    let mut pending_space = false;

    for token in tokens {
        match token {
            WrapToken::Space => pending_space = !current.is_empty(),
            WrapToken::Break => {
                atom_lines.push(std::mem::take(&mut current));
                current_width = 0;
                pending_space = false;
            }
            WrapToken::Word(word) => {
                let word_width = word.iter().map(|atom| atom.width).sum::<usize>();
                let separator = usize::from(pending_space && !current.is_empty());
                if !current.is_empty() && current_width + separator + word_width > width {
                    atom_lines.push(std::mem::take(&mut current));
                    current_width = 0;
                }
                if pending_space && !current.is_empty() {
                    let source = word.first().map_or(0..0, |atom| atom.source.clone());
                    current.push(Atom {
                        text: " ".to_owned(),
                        width: 1,
                        style: space_style,
                        source,
                        link: None,
                    });
                    current_width += 1;
                }
                pending_space = false;
                for atom in word {
                    if !current.is_empty() && current_width + atom.width > width {
                        atom_lines.push(std::mem::take(&mut current));
                        current_width = 0;
                    }
                    current_width += atom.width;
                    current.push(atom);
                }
            }
        }
    }
    if !current.is_empty() || atom_lines.is_empty() {
        atom_lines.push(current);
    }

    let mut output = RenderedDocument::default();
    for atoms in atom_lines {
        push_atom_line(&mut output, atoms);
    }
    output
}

fn wrap_tokens(runs: &[InlineRun]) -> Vec<WrapToken> {
    let mut tokens = Vec::new();
    let mut word = Vec::new();
    for run in runs {
        if run.hard_break {
            flush_word(&mut tokens, &mut word);
            tokens.push(WrapToken::Break);
            continue;
        }
        for grapheme in run.text.graphemes(true) {
            if grapheme == "\n" {
                flush_word(&mut tokens, &mut word);
                tokens.push(WrapToken::Break);
            } else if grapheme.chars().all(char::is_whitespace) {
                flush_word(&mut tokens, &mut word);
                if !matches!(tokens.last(), Some(WrapToken::Space | WrapToken::Break)) {
                    tokens.push(WrapToken::Space);
                }
            } else {
                word.push(Atom {
                    text: grapheme.to_owned(),
                    width: display_width(grapheme),
                    style: run.style,
                    source: run.source.clone(),
                    link: run.link.clone(),
                });
            }
        }
    }
    flush_word(&mut tokens, &mut word);
    tokens
}

fn flush_word(tokens: &mut Vec<WrapToken>, word: &mut Vec<Atom>) {
    if !word.is_empty() {
        tokens.push(WrapToken::Word(std::mem::take(word)));
    }
}

fn push_atom_line(output: &mut RenderedDocument, atoms: Vec<Atom>) {
    let line_index = output.lines.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut line_source: Option<Range<usize>> = None;
    let mut column = 0;
    let mut active_link: Option<(String, Range<usize>, usize, usize)> = None;

    for atom in atoms {
        merge_source(&mut line_source, &atom.source);
        match &atom.link {
            Some(destination) => match &mut active_link {
                Some((active, source, _start, end)) if active == destination && *end == column => {
                    merge_range(source, &atom.source);
                    *end += atom.width;
                }
                _ => {
                    flush_link(output, line_index, active_link.take());
                    active_link = Some((
                        destination.clone(),
                        atom.source.clone(),
                        column,
                        column + atom.width,
                    ));
                }
            },
            None => flush_link(output, line_index, active_link.take()),
        }
        if let Some(last) = spans.last_mut().filter(|span| span.style == atom.style) {
            last.content.to_mut().push_str(&atom.text);
        } else {
            spans.push(Span::styled(atom.text, atom.style));
        }
        column += atom.width;
    }
    flush_link(output, line_index, active_link.take());
    output.lines.push(Line::from(spans));
    output.line_sources.push(line_source);
}

fn flush_link(
    output: &mut RenderedDocument,
    line_index: usize,
    link: Option<(String, Range<usize>, usize, usize)>,
) {
    let Some((destination, source, start, end)) = link else {
        return;
    };
    output.hyperlinks.push(HyperlinkRegion {
        line_index,
        columns: start..end,
        destination,
        source,
    });
}

fn table_rows(nodes: &[Node], styles: MarkdownStyles) -> Vec<TableRow> {
    let mut rows = Vec::new();
    for node in nodes {
        match &node.kind {
            NodeKind::Element {
                element: ElementKind::TableHead,
                children,
            } => rows.push(TableRow {
                cells: table_cells(children, styles, true),
                source: node.source.clone(),
                header: true,
            }),
            NodeKind::Element {
                element: ElementKind::TableRow,
                children,
            } => rows.push(TableRow {
                cells: table_cells(children, styles, false),
                source: node.source.clone(),
                header: false,
            }),
            _ => {}
        }
    }
    rows
}

fn table_cells(nodes: &[Node], styles: MarkdownStyles, header: bool) -> Vec<TableCell> {
    nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::Element {
                element: ElementKind::TableCell,
                children,
            } => Some(TableCell {
                runs: collect_inlines(
                    children,
                    styles,
                    if header {
                        styles.text.add_modifier(Modifier::BOLD)
                    } else {
                        styles.text
                    },
                ),
                source: node.source.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn fit_columns(mut widths: Vec<usize>, available: usize) -> Vec<usize> {
    while widths.iter().sum::<usize>() > available {
        let Some((index, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, width)| **width > 3)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[index] -= 1;
    }
    widths
}

#[derive(Clone, Copy)]
enum BorderRow {
    Top,
    Middle,
    Bottom,
}

fn table_border(widths: &[usize], ascii: bool, row: BorderRow) -> String {
    if ascii {
        return format!(
            "+{}+",
            widths
                .iter()
                .map(|width| "-".repeat(width + 2))
                .collect::<Vec<_>>()
                .join("+")
        );
    }
    let (left, join, right) = match row {
        BorderRow::Top => ('┌', '┬', '┐'),
        BorderRow::Middle => ('├', '┼', '┤'),
        BorderRow::Bottom => ('└', '┴', '┘'),
    };
    format!(
        "{left}{}{right}",
        widths
            .iter()
            .map(|width| "─".repeat(width + 2))
            .collect::<Vec<_>>()
            .join(&join.to_string())
    )
}

pub(super) fn append_document(
    destination: &mut RenderedDocument,
    mut source: RenderedDocument,
    blank: bool,
) {
    if blank {
        destination.lines.push(Line::default());
        destination.line_sources.push(None);
    }
    let line_offset = destination.lines.len();
    for link in &mut source.hyperlinks {
        link.line_index += line_offset;
    }
    for block in &mut source.code_blocks {
        block.output_lines =
            (block.output_lines.start + line_offset)..(block.output_lines.end + line_offset);
    }
    for block in &mut source.blocks {
        block.output_lines =
            (block.output_lines.start + line_offset)..(block.output_lines.end + line_offset);
    }
    destination.lines.append(&mut source.lines);
    destination.line_sources.append(&mut source.line_sources);
    destination.hyperlinks.append(&mut source.hyperlinks);
    destination.code_blocks.append(&mut source.code_blocks);
    destination.blocks.append(&mut source.blocks);
}

fn prepend_lines(output: &mut RenderedDocument, prefix: &[Span<'static>], prefix_width: usize) {
    prepend_first_and_rest(output, prefix, prefix, prefix_width);
}

fn prepend_first_and_rest(
    output: &mut RenderedDocument,
    first: &[Span<'static>],
    rest: &[Span<'static>],
    prefix_width: usize,
) {
    for (index, line) in output.lines.iter_mut().enumerate() {
        let mut spans = if index == 0 {
            first.to_owned()
        } else {
            rest.to_owned()
        };
        spans.append(&mut line.spans);
        line.spans = spans;
    }
    for link in &mut output.hyperlinks {
        link.columns = (link.columns.start + prefix_width)..(link.columns.end + prefix_width);
    }
}

fn shift_metadata_lines(output: &mut RenderedDocument, amount: usize) {
    for link in &mut output.hyperlinks {
        link.line_index += amount;
    }
    for code in &mut output.code_blocks {
        code.output_lines = (code.output_lines.start + amount)..(code.output_lines.end + amount);
    }
    for block in &mut output.blocks {
        block.output_lines = (block.output_lines.start + amount)..(block.output_lines.end + amount);
    }
}

fn record_block(output: &mut RenderedDocument, kind: RenderedBlockKind, source: Range<usize>) {
    if !output.is_empty() {
        output.blocks.push(RenderedBlockRegion {
            kind,
            output_lines: 0..output.lines.len(),
            source,
        });
    }
}

fn push_line(
    output: &mut RenderedDocument,
    spans: Vec<Span<'static>>,
    source: Option<Range<usize>>,
) {
    output.lines.push(Line::from(spans));
    output.line_sources.push(source);
}

fn clip_spans(spans: &[Span<'static>], width: usize, fallback: Style) -> Vec<Span<'static>> {
    let total = spans_width(spans);
    if total <= width {
        return spans.to_vec();
    }
    let target = width.saturating_sub(1);
    let mut output: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    let mut last_style = fallback;
    'outer: for span in spans {
        last_style = span.style;
        for grapheme in span.content.graphemes(true) {
            let grapheme_width = display_width(grapheme);
            if used + grapheme_width > target {
                break 'outer;
            }
            if let Some(last) = output.last_mut().filter(|last| last.style == span.style) {
                last.content.to_mut().push_str(grapheme);
            } else {
                output.push(Span::styled(grapheme.to_owned(), span.style));
            }
            used += grapheme_width;
        }
    }
    if width > 0 {
        output.push(Span::styled("…", last_style));
    }
    output
}

fn clip_plain(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    let target = width.saturating_sub(1);
    let mut output = String::new();
    let mut used = 0;
    for grapheme in text.graphemes(true) {
        let next = display_width(grapheme);
        if used + next > target {
            break;
        }
        output.push_str(grapheme);
        used += next;
    }
    if width > 0 {
        output.push('…');
    }
    output
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn plain_text(nodes: &[Node]) -> String {
    let mut output = String::new();
    for node in nodes {
        match &node.kind {
            NodeKind::Leaf(
                LeafKind::Text(text)
                | LeafKind::Html(text)
                | LeafKind::Code(text)
                | LeafKind::InlineMath(text)
                | LeafKind::DisplayMath(text),
            ) => {
                output.push_str(&clean_text(text));
            }
            NodeKind::Leaf(LeafKind::SoftBreak | LeafKind::HardBreak) => output.push('\n'),
            NodeKind::Leaf(LeafKind::FootnoteReference(label)) => {
                let _ = write!(output, "[^{label}]");
            }
            NodeKind::Leaf(LeafKind::TaskListMarker(checked)) => {
                output.push_str(if *checked { "[x] " } else { "[ ] " });
            }
            NodeKind::Leaf(LeafKind::Rule) => {}
            NodeKind::Element { children, .. } => output.push_str(&plain_text(children)),
        }
    }
    output
}

fn body_source_range(children: &[Node]) -> Option<Range<usize>> {
    let mut source = None;
    for node in children {
        merge_source(&mut source, &node.source);
    }
    source
}

fn inline_plain(runs: &[InlineRun]) -> String {
    runs.iter().map(|run| run.text.as_str()).collect::<String>()
}

fn inline_plain_width(runs: &[InlineRun]) -> usize {
    display_width(&inline_plain(runs))
}

fn display_width(text: &str) -> usize {
    text.width()
}

fn clean_text(text: &str) -> String {
    text.chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect::<String>()
        .replace('\t', "    ")
}

fn safe_destination(destination: &str) -> Option<String> {
    if destination.chars().any(char::is_control) {
        return None;
    }
    let trimmed = destination.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || (!lower.contains(':') && !trimmed.is_empty())
    {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

fn merge_source(target: &mut Option<Range<usize>>, source: &Range<usize>) {
    if let Some(target) = target {
        merge_range(target, source);
    } else {
        *target = Some(source.clone());
    }
}

fn merge_range(target: &mut Range<usize>, source: &Range<usize>) {
    target.start = cmp::min(target.start, source.start);
    target.end = cmp::max(target.end, source.end);
}

fn stable_prefix(source: &str, document: &Document) -> usize {
    // Keep the newest block mutable even when it currently appears complete.
    // A trailing fenced block may still receive its closing delimiter, and a
    // list can continue with the next streamed chunk. Completed messages are
    // cached as a whole by the retained renderer, so this conservatism is cheap.
    document
        .nodes
        .iter()
        .rev()
        .nth(1)
        .map_or(0, |node| node.source.end.min(source.len()))
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier};

    use super::*;
    use crate::tui::markdown::parse;

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
            link: Style::default()
                .fg(Color::LightMagenta)
                .add_modifier(Modifier::UNDERLINED),
            warning: Style::default().fg(Color::Yellow),
            success: Style::default().fg(Color::Green),
            truecolor: false,
        }
    }

    fn rendered(source: &str, width: usize) -> RenderedDocument {
        render(source, &parse::parse(source), width, styles(), false)
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
    fn removes_markdown_delimiters_and_preserves_styles() {
        let output = rendered("**bold** and *soft* with `code`", 80);
        assert_eq!(plain(&output), ["bold and soft with code"]);
        assert!(output.lines[0].spans.iter().any(|span| {
            span.content == "bold" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        assert!(output.lines[0].spans.iter().any(|span| {
            span.content == "soft" && span.style.add_modifier.contains(Modifier::ITALIC)
        }));
    }

    #[test]
    fn wraps_styled_unicode_without_crossing_width() {
        let output = rendered(
            "A **wide 👋 blossom** and a-very-long-unbreakable-token",
            14,
        );
        assert!(output.lines.iter().all(|line| line.width() <= 14));
        assert!(plain(&output).len() > 2);
    }

    #[test]
    fn emits_link_and_code_block_metadata() {
        let source = "[site](https://example.com)\n\n```rust\nfn main() {}\n```\n";
        let output = rendered(source, 60);
        assert_eq!(output.hyperlinks.len(), 1);
        assert_eq!(output.hyperlinks[0].destination, "https://example.com");
        assert_eq!(output.code_blocks.len(), 1);
        assert_eq!(output.code_blocks[0].info, "rust");
        assert_eq!(output.code_blocks[0].body, "fn main() {}\n");
    }

    #[test]
    fn unsafe_link_schemes_are_not_activated() {
        let output = rendered("[bad](javascript:alert(1))", 80);
        assert!(output.hyperlinks.is_empty());
        assert_eq!(plain(&output), ["bad"]);
    }

    #[test]
    fn tables_fit_or_fall_back_to_records() {
        let source = "| Name | State |\n|---|---|\n| blossom | ready |\n";
        let wide = plain(&rendered(source, 40)).join("\n");
        assert!(wide.contains('┌') && wide.contains("blossom"));
        let narrow = plain(&rendered(source, 12)).join("\n");
        assert!(narrow.contains("Name:") && narrow.contains("blos") && narrow.contains("som"));
    }

    #[test]
    fn gfm_checkbox_stays_on_the_same_line_as_its_item() {
        let output = rendered("- [x] preserve the raw response\n", 40);
        assert_eq!(plain(&output), ["• [✓] preserve the raw response"]);
    }

    #[test]
    fn final_streamed_source_matches_one_shot_render() {
        let source = "## Result\n\n- one\n- **two**\n\n> safe\n";
        let expected = plain(&rendered(source, 24));
        let mut streamed = String::new();
        for chunk in source.as_bytes().chunks(3) {
            streamed.push_str(std::str::from_utf8(chunk).expect("utf8 chunk"));
            let _partial = rendered(&streamed, 24);
        }
        assert_eq!(plain(&rendered(&streamed, 24)), expected);
    }
}
