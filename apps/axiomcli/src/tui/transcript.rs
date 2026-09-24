//! Transcript and task-card presentation.

use std::time::Duration;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::{
    BODY_INDENT,
    markdown::MarkdownStyles,
    state::{Entry, EntryFormat, EntryKind, TuiState},
    task::{TaskPhase, TaskRunView, TaskTimelineItem, ToolPhase},
    text::{format_elapsed, sanitize_terminal_text, wrap_to_width},
    theme::{ColorMode, Theme},
};

pub(super) fn markdown_styles(theme: Theme, mode: ColorMode) -> MarkdownStyles {
    MarkdownStyles {
        text: Style::default().fg(theme.text),
        muted: Style::default().fg(theme.muted),
        accent: Style::default().fg(theme.text),
        accent_bright: Style::default().fg(theme.text),
        code: Style::default().fg(theme.text).bg(theme.surface),
        code_surface: Style::default().fg(theme.text).bg(theme.surface),
        border: Style::default().fg(theme.border),
        quote: Style::default().fg(theme.muted),
        link: Style::default()
            .fg(theme.text)
            .add_modifier(Modifier::UNDERLINED),
        warning: Style::default().fg(theme.warning),
        success: Style::default().fg(theme.success),
        truecolor: mode == ColorMode::TrueColor && theme.base != Color::Reset,
    }
}

impl TuiState {
    /// Builds the transcript for a known content width so wrapping happens here
    /// rather than in `Paragraph`: continuation lines keep the body indent, and
    /// wide glyphs are measured by display width instead of by `char` count.
    pub(super) fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        let theme = self.options.theme();
        let body_width = usize::from(width).saturating_sub(BODY_INDENT).max(4);
        let indent = " ".repeat(BODY_INDENT);
        let mut lines = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            if let Some(task) = &entry.task {
                lines.extend(self.task_lines(index, entry, task, width, theme));
                continue;
            }
            let entry_start = lines.len();
            let (marker, color, label) = match entry.kind {
                EntryKind::User => (self.glyph("›", ">"), theme.muted, "You"),
                EntryKind::Assistant | EntryKind::Task => {
                    (self.glyph("·", "*"), theme.muted, "Axiom")
                }
                EntryKind::Reasoning => (self.glyph("◇", "~"), theme.muted, "Thinking"),
                EntryKind::Tool => (self.glyph("▸", "-"), theme.muted, "Tool"),
                EntryKind::System => (self.glyph("·", "+"), theme.muted, "Notice"),
                EntryKind::Error => (self.glyph("▲", "!"), theme.error, "Error"),
            };
            let mut label_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            if self.selected == Some(index) {
                label_style = label_style.fg(theme.text).patch(theme.selection());
            }
            let mut header = vec![
                Span::styled(marker, Style::default().fg(color)),
                Span::raw(" "),
                Span::styled(label, label_style),
            ];
            let mut tags = String::new();
            if entry.collapsed {
                tags.push_str("  collapsed");
            }
            if self.search_matches.contains(&index) {
                tags.push_str("  match");
            }
            if !tags.is_empty() {
                header.push(Span::styled(tags, Style::default().fg(theme.muted)));
            }
            lines.push(Line::from(header));

            let body_style = match entry.kind {
                EntryKind::Reasoning | EntryKind::System => Style::default().fg(theme.muted),
                EntryKind::Error => Style::default().fg(theme.error),
                _ => Style::default().fg(theme.text),
            };
            let body = if entry.collapsed {
                entry.text.lines().take(3).collect::<Vec<_>>().join("\n")
            } else {
                entry.text.clone()
            };
            if entry.format == EntryFormat::Markdown {
                let rendered_lines = entry
                    .markdown
                    .borrow_mut()
                    .render(
                        &body,
                        body_width,
                        markdown_styles(theme, self.options.color_mode),
                        self.options.ascii,
                    )
                    .lines
                    .clone();
                for mut rendered_line in rendered_lines {
                    let mut spans = vec![Span::raw(indent.clone())];
                    spans.append(&mut rendered_line.spans);
                    lines.push(Line::from(spans));
                }
            } else {
                for source in body.lines() {
                    for wrapped in wrap_to_width(source, body_width) {
                        lines.push(Line::from(Span::styled(
                            format!("{indent}{wrapped}"),
                            body_style,
                        )));
                    }
                }
            }
            if entry.collapsed {
                lines.push(Line::from(Span::styled(
                    format!("{indent}… select and press Enter to expand, or v to view"),
                    Style::default().fg(theme.muted),
                )));
            }
            if entry.kind == EntryKind::User {
                for line in &mut lines[entry_start..] {
                    line.style = line.style.bg(theme.surface);
                }
            }
        }
        lines
    }

    pub(super) fn task_lines(
        &self,
        index: usize,
        entry: &Entry,
        task: &TaskRunView,
        width: u16,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let (marker, marker_color, state_label) = match task.phase {
            TaskPhase::Starting | TaskPhase::Running => {
                (self.activity_glyph(false), theme.brand, "Working")
            }
            TaskPhase::Waiting => (self.glyph("◇", "o"), theme.text, "Waiting for you"),
            TaskPhase::Completed => (self.completion_glyph(), theme.success, "Complete"),
            TaskPhase::Failed => (self.glyph("✕", "x"), theme.error, "Stopped with an error"),
            TaskPhase::Cancelled => (self.glyph("■", "x"), theme.warning, "Cancelled"),
        };
        let mut label_style = Style::default()
            .fg(theme.muted)
            .add_modifier(Modifier::BOLD);
        if self.selected == Some(index) {
            label_style = label_style.fg(theme.text).patch(theme.selection());
        }
        let mut header = vec![
            Span::styled(marker, Style::default().fg(marker_color)),
            Span::raw(" "),
            Span::styled("Axiom", label_style),
            Span::styled(format!("  {state_label}"), Style::default().fg(theme.muted)),
        ];
        if task.phase.is_terminal() && task.elapsed() >= Duration::from_secs(1) {
            header.push(Span::styled(
                format!(" · {}", format_elapsed(task.elapsed())),
                Style::default().fg(theme.muted),
            ));
        }
        if self.search_matches.contains(&index) {
            header.push(Span::styled("  match", Style::default().fg(theme.muted)));
        }
        lines.push(Line::from(header));

        let body_width = usize::from(width).saturating_sub(4).max(4);
        let mut rail_row = 0_u64;
        if matches!(
            task.phase,
            TaskPhase::Starting | TaskPhase::Running | TaskPhase::Waiting
        ) {
            for item in task.tasks.iter().take(4) {
                let (glyph, color) = match item.status {
                    crate::app::TaskStatus::Pending => (self.glyph("○", "o"), theme.muted),
                    crate::app::TaskStatus::InProgress => (self.activity_glyph(false), theme.text),
                    crate::app::TaskStatus::Completed => (self.glyph("✓", "+"), theme.success),
                };
                self.push_task_body(
                    &mut lines,
                    task,
                    &format!("{glyph} {}", item.title),
                    Style::default().fg(color),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
        }

        if !task.tasks.is_empty() && !task.timeline.is_empty() {
            self.push_task_body(
                &mut lines,
                task,
                "",
                Style::default(),
                body_width,
                theme,
                rail_row,
            );
            rail_row = rail_row.saturating_add(1);
        }

        let mut previous_was_tool = None;
        for item in &task.timeline {
            let is_tool = matches!(item, TaskTimelineItem::Tool { .. });
            if previous_was_tool.is_some_and(|previous| is_tool != previous) {
                self.push_task_body(
                    &mut lines,
                    task,
                    "",
                    Style::default(),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            match item {
                TaskTimelineItem::Response { text, markdown } => {
                    let rendered = markdown
                        .borrow_mut()
                        .render(
                            text,
                            body_width,
                            markdown_styles(theme, self.options.color_mode),
                            self.options.ascii,
                        )
                        .lines
                        .clone();
                    for mut line in rendered {
                        let mut spans = vec![Span::raw("    ")];
                        spans.append(&mut line.spans);
                        lines.push(Line::from(spans));
                        rail_row = rail_row.saturating_add(1);
                    }
                }
                TaskTimelineItem::Tool { call_id } => {
                    if let Some(tool) = task.tools.iter().find(|tool| tool.call_id == *call_id) {
                        let (glyph, color) = match tool.phase {
                            ToolPhase::Proposed | ToolPhase::Running => {
                                (self.activity_glyph(false), theme.text)
                            }
                            ToolPhase::Completed => (self.glyph("✓", "+"), theme.muted),
                            ToolPhase::Failed => (self.glyph("✕", "x"), theme.error),
                            ToolPhase::Cancelled => (self.glyph("■", "-"), theme.warning),
                        };
                        self.push_task_body(
                            &mut lines,
                            task,
                            &format!(
                                "{glyph} {}",
                                sanitize_terminal_text(&tool.timeline_label(&self.cwd))
                            ),
                            Style::default().fg(color),
                            body_width,
                            theme,
                            rail_row,
                        );
                        rail_row = rail_row.saturating_add(1);
                    }
                }
            }
            previous_was_tool = Some(is_tool);
        }

        match task.phase {
            TaskPhase::Starting | TaskPhase::Running | TaskPhase::Waiting
                if task.running_tool().is_none() =>
            {
                if !task.timeline.is_empty() {
                    self.push_task_body(
                        &mut lines,
                        task,
                        "",
                        Style::default(),
                        body_width,
                        theme,
                        rail_row,
                    );
                    rail_row = rail_row.saturating_add(1);
                }
                let waiting = task.phase == TaskPhase::Waiting;
                self.push_task_body(
                    &mut lines,
                    task,
                    &format!("{} {}", self.activity_glyph(waiting), task.activity.label()),
                    Style::default().fg(theme.muted),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            TaskPhase::Completed if task.timeline.is_empty() => {
                self.push_task_body(
                    &mut lines,
                    task,
                    &format!("{} Finished the request", self.glyph("✓", "+")),
                    Style::default().fg(theme.success),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            TaskPhase::Failed | TaskPhase::Cancelled => {
                if !task.timeline.is_empty() {
                    self.push_task_body(
                        &mut lines,
                        task,
                        "",
                        Style::default(),
                        body_width,
                        theme,
                        rail_row,
                    );
                    rail_row = rail_row.saturating_add(1);
                }
                let (message, color) = if task.phase == TaskPhase::Failed {
                    (
                        task.error.as_deref().unwrap_or("The task failed"),
                        theme.error,
                    )
                } else {
                    ("Work stopped before completion", theme.warning)
                };
                self.push_task_body(
                    &mut lines,
                    task,
                    message,
                    Style::default().fg(color),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            _ => {}
        }

        if task.phase == TaskPhase::Completed
            && self.task_index(&task.turn_id) == Some(index)
            && self.response_hit_output_limit(&task.turn_id)
        {
            self.push_task_body(
                &mut lines,
                task,
                "The model reached its output limit. You can ask it to continue.",
                Style::default().fg(theme.warning),
                body_width,
                theme,
                rail_row,
            );
            rail_row = rail_row.saturating_add(1);
        }

        if !entry.collapsed && (!task.tools.is_empty() || !task.reasoning.trim().is_empty()) {
            self.push_task_body(
                &mut lines,
                task,
                "",
                Style::default(),
                body_width,
                theme,
                rail_row,
            );
            rail_row = rail_row.saturating_add(1);
            self.push_task_body(
                &mut lines,
                task,
                "Details",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                body_width,
                theme,
                rail_row,
            );
            rail_row = rail_row.saturating_add(1);
            if !task.reasoning.trim().is_empty() {
                self.push_task_body(
                    &mut lines,
                    task,
                    &format!("Thought\n{}", task.reasoning.trim()),
                    Style::default()
                        .fg(theme.muted)
                        .add_modifier(Modifier::ITALIC),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            for tool in &task.tools {
                let detail = format!(
                    "{} · {} · {:?}{}{}",
                    tool.name,
                    tool.call_id,
                    tool.phase,
                    if tool.output.is_empty() {
                        String::new()
                    } else {
                        format!("\n{}", tool.output)
                    },
                    if tool.diff.is_empty() {
                        String::new()
                    } else {
                        format!("\nDIFF\n{}", tool.diff)
                    }
                );
                self.push_task_body(
                    &mut lines,
                    task,
                    &detail,
                    Style::default().fg(theme.muted),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
        }

        lines
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn push_task_body(
        &self,
        lines: &mut Vec<Line<'static>>,
        task: &TaskRunView,
        text: &str,
        style: Style,
        width: usize,
        theme: Theme,
        row: u64,
    ) {
        let mut offset = 0_u64;
        for source in text.lines() {
            for wrapped in wrap_to_width(source, width) {
                let mut spans = self.task_rail_spans(task, theme, row.saturating_add(offset));
                spans.push(Span::styled(wrapped, style));
                lines.push(Line::from(spans));
                offset = offset.saturating_add(1);
            }
        }
        if text.is_empty() {
            lines.push(Line::from(self.task_rail_spans(task, theme, row)));
        }
    }

    pub(super) fn task_rail_spans(
        &self,
        task: &TaskRunView,
        theme: Theme,
        row: u64,
    ) -> Vec<Span<'static>> {
        let color = match task.phase {
            TaskPhase::Starting | TaskPhase::Running => {
                self.activity_color(theme, row.saturating_mul(2))
            }
            TaskPhase::Waiting | TaskPhase::Completed => theme.border,
            TaskPhase::Failed => theme.error,
            TaskPhase::Cancelled => theme.warning,
        };
        vec![
            Span::raw("  "),
            Span::styled(self.glyph("│", "|").to_owned(), Style::default().fg(color)),
            Span::raw(" "),
        ]
    }

    pub(super) fn activity_frame(&self, offset: u64) -> (&'static str, &'static str, usize) {
        const FRAMES: [(&str, &str); 8] = [
            ("·", "."),
            ("·", "."),
            ("•", "o"),
            ("●", "O"),
            ("●", "O"),
            ("•", "o"),
            ("·", "."),
            ("·", "."),
        ];
        let index =
            usize::try_from(self.animation_frame.saturating_add(offset) % 8).unwrap_or_default();
        (FRAMES[index].0, FRAMES[index].1, index)
    }

    pub(super) fn activity_glyph(&self, waiting: bool) -> &'static str {
        if waiting {
            return self.glyph("○", "o");
        }
        if !self.options.animation {
            return self.glyph("●", "*");
        }
        let (unicode, ascii, _) = self.activity_frame(0);
        self.glyph(unicode, ascii)
    }

    pub(super) fn activity_color(&self, theme: Theme, offset: u64) -> Color {
        if !self.options.animation || matches!(self.activity_frame(offset).2, 2..=5) {
            theme.brand
        } else {
            theme.border
        }
    }

    pub(super) fn completion_glyph(&self) -> &'static str {
        self.glyph("✓", "+")
    }

    pub(super) fn compaction_frame(&self) -> &'static str {
        const UNICODE: [&str; 8] = [
            "[▰   ▰   ▰]",
            "[ ▰  ▰  ▰ ]",
            "[  ▰ ▰ ▰  ]",
            "[   ▰▰▰   ]",
            "[    ◆    ]",
            "[    ◆    ]",
            "[    ·    ]",
            "[         ]",
        ];
        const ASCII: [&str; 8] = [
            "[#   #   #]",
            "[ #  #  # ]",
            "[  # # #  ]",
            "[   ###   ]",
            "[    @    ]",
            "[    @    ]",
            "[    .    ]",
            "[         ]",
        ];
        let index = if self.options.animation {
            usize::try_from(self.animation_frame % 8).unwrap_or_default()
        } else {
            4
        };
        self.glyph(UNICODE[index], ASCII[index])
    }
}
