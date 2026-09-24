//! Terminal layout and overlay rendering.

use std::env;

use axiom_secure_client::SecurityEvidence;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    app::{PermissionProfile, ThinkingLevel},
    policy::describe_effect,
    slash::{self},
};

use super::{
    COMPOSER_MAX_CONTENT_ROWS, GUTTER, MIN_HEIGHT, MIN_WIDTH,
    attestation::{
        MAX_WORKLOAD_DISPLAY_CHARS, ReportStatus, attestation_picker_directories,
        bounded_workload_display, humanize_evidence_key,
    },
    brand,
    state::{
        AuthOverlay, Focus, InteractionView, Overlay, SavePickerFocus, SecurityView, StatusTone,
        TuiState,
    },
    task::TaskRunView,
    text::{
        centered_rect, display_width, fitted_rect, sanitize_terminal_text, tail_to_width,
        truncate_end, truncate_middle,
    },
    theme::Theme,
};

pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    let theme = state.options.theme();
    // Paint the whole frame first so every region sits on one slab instead of
    // letting the host terminal's own background show through between widgets.
    frame.render_widget(
        Block::default().style(Style::default().fg(theme.text).bg(theme.base)),
        area,
    );
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        frame.render_widget(
            Paragraph::new(if state.options.ascii {
                "AxiomCLI needs at least 42x10 terminal cells."
            } else {
                "AxiomCLI needs at least 42×10 terminal cells."
            })
            .style(Style::default().fg(theme.text))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let rows = main_rows(area, state);
    render_header(frame, state, theme, rows[0]);
    render_transcript(frame, state, theme, rows[1]);
    render_status(frame, state, theme, rows[2]);
    render_composer(frame, state, theme, rows[3]);
    render_hints(frame, state, theme, rows[4]);

    render_slash_suggestions(frame, state, theme, rows[1]);

    if let Some(overlay) = &state.overlay {
        render_overlay(frame, state, overlay, theme);
    }
    // Foreground questions and approvals receive keyboard input first, so they
    // must also remain visible above an inspector opened during active work.
    if state.interaction.is_some() {
        render_interaction(frame, state, theme);
    }
}

pub(super) fn render_slash_suggestions(
    frame: &mut Frame<'_>,
    state: &TuiState,
    theme: Theme,
    area: Rect,
) {
    if state.focus != Focus::Composer
        || state.interaction.is_some()
        || state.overlay.is_some()
        || state.search_active
        || !state.input.starts_with('/')
    {
        return;
    }
    let candidates = state.slash_candidates();
    let argument_mode = candidates
        .first()
        .is_some_and(|candidate| !candidate.label.starts_with('/'));
    let title = if argument_mode {
        let command = state
            .input
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("command");
        format!(" /{command} options · ↑↓ select · Enter/Tab accept ")
    } else {
        " / commands · ↑↓ select · Enter/Tab accept ".into()
    };
    if candidates.is_empty() || area.width <= GUTTER.saturating_mul(2) + 8 || area.height < 3 {
        return;
    }
    let available_width = area.width.saturating_sub(GUTTER.saturating_mul(2));
    let width = available_width;
    let height = u16::try_from(candidates.len().saturating_mul(2).saturating_add(2))
        .unwrap_or(u16::MAX)
        .min(area.height);
    let popup = Rect {
        x: area.x.saturating_add(GUTTER),
        y: area.y.saturating_add(area.height.saturating_sub(height)),
        width,
        height,
    };
    let selected = state.slash_selection.min(candidates.len() - 1);
    let visible_items = usize::from(height.saturating_sub(2) / 2).max(1);
    let start = selected
        .saturating_sub(visible_items.saturating_sub(1))
        .min(candidates.len().saturating_sub(visible_items));
    let lines = candidates
        .into_iter()
        .enumerate()
        .skip(start)
        .take(visible_items)
        .flat_map(|(index, completion)| {
            let is_selected = index == selected;
            let label = if argument_mode {
                completion.label
            } else {
                slash::COMMANDS
                    .iter()
                    .find(|spec| completion.label == format!("/{}", spec.name))
                    .map_or(completion.label, |spec| spec.usage.to_owned())
            };
            let marker = if is_selected {
                state.glyph("›", ">")
            } else {
                " "
            };
            let row_background = is_selected.then_some(theme.selection_bg);
            let selection = if is_selected {
                theme.selection()
            } else {
                Style::default()
            };
            let label_style = Style::default()
                .fg(theme.text)
                .bg(row_background.unwrap_or(theme.surface))
                .patch(selection)
                .add_modifier(Modifier::BOLD);
            let description_style = Style::default()
                .fg(if is_selected { theme.text } else { theme.muted })
                .bg(row_background.unwrap_or(theme.surface))
                .patch(selection);
            [
                Line::from(Span::styled(format!("{marker} {label}"), label_style)),
                Line::from(Span::styled(
                    format!("  {}", completion.description),
                    description_style,
                )),
            ]
        })
        .collect::<Vec<_>>();
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .style(Style::default().bg(theme.surface)),
            )
            .style(Style::default().bg(theme.surface)),
        popup,
    );
}

pub(super) fn composer_text_width(terminal_width: u16) -> u16 {
    terminal_width
        .saturating_sub(GUTTER.saturating_mul(2).saturating_add(5))
        .max(1)
}

pub(super) fn composer_content_height(area: Rect, state: &TuiState) -> u16 {
    if state.interaction.is_some() || state.answer_prompt.is_some() {
        return 1;
    }
    let proportional_cap = (area.height / 4).max(1);
    let transcript_safe_cap = area.height.saturating_sub(11).max(1);
    state
        .input
        .desired_height(composer_text_width(area.width))
        .clamp(
            1,
            COMPOSER_MAX_CONTENT_ROWS
                .min(proportional_cap)
                .min(transcript_safe_cap),
        )
}

pub(super) fn main_rows(area: Rect, state: &TuiState) -> [Rect; 5] {
    let composer_height =
        composer_content_height(area, state).saturating_add(if area.height >= 14 { 4 } else { 2 });
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);
    [rows[0], rows[1], rows[2], rows[3], rows[4]]
}

pub(super) fn transcript_max_scroll(state: &TuiState, terminal_area: Rect) -> u16 {
    if terminal_area.width < MIN_WIDTH || terminal_area.height < MIN_HEIGHT {
        return 0;
    }
    let transcript_area = main_rows(terminal_area, state)[1];
    let inner = Block::default()
        .padding(Padding::new(GUTTER, GUTTER, 1, 1))
        .inner(transcript_area);
    let visible = usize::from(inner.height);
    let lines = state.transcript_lines(inner.width).len().max(visible);
    u16::try_from(lines.saturating_sub(visible)).unwrap_or(u16::MAX)
}

pub(super) fn scroll_transcript_up(state: &mut TuiState, terminal_area: Rect, amount: u16) {
    let max_scroll = transcript_max_scroll(state, terminal_area);
    if max_scroll == 0 {
        return;
    }
    if state.follow_output {
        state.scroll = max_scroll;
        state.follow_output = false;
    } else {
        state.scroll = state.scroll.min(max_scroll);
    }
    state.scroll = state.scroll.saturating_sub(amount);
}

pub(super) fn scroll_transcript_down(state: &mut TuiState, terminal_area: Rect, amount: u16) {
    if state.follow_output {
        return;
    }
    let max_scroll = transcript_max_scroll(state, terminal_area);
    state.scroll = state.scroll.saturating_add(amount).min(max_scroll);
    if state.scroll == max_scroll {
        state.follow_output = true;
        state.scroll = 0;
    }
}

/// Trims `inset` columns from both sides, or `None` when nothing would be left.
pub(super) fn inset_horizontal(area: Rect, inset: u16) -> Option<Rect> {
    let width = area.width.checked_sub(inset.saturating_mul(2))?;
    (width > 0).then(|| Rect {
        x: area.x.saturating_add(inset),
        y: area.y,
        width,
        height: area.height,
    })
}

pub(super) fn render_header(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::horizontal(GUTTER));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    let report = state.report_status();
    let (marker, color) = match report {
        ReportStatus::Verified => (state.glyph("✓", "+"), theme.success),
        ReportStatus::Verifying => (state.activity_glyph(false), theme.brand),
        ReportStatus::Failed => (state.glyph("✕", "!"), theme.error),
        ReportStatus::Degraded | ReportStatus::Outdated => (state.glyph("⚠", "!"), theme.warning),
        ReportStatus::Expired | ReportStatus::Unavailable => (state.glyph("○", "o"), theme.warning),
        ReportStatus::Idle | ReportStatus::Unverified => (state.glyph("○", "o"), theme.muted),
    };
    let security = format!("{marker} {}", report.label());
    let width = usize::from(inner.width);
    let mut left = vec![Span::styled(
        "Axiom",
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    )];
    let mut left_width = 5;
    let cwd_budget = width.saturating_sub(display_width(&security) + left_width + 5);
    if cwd_budget >= 8 {
        let cwd = truncate_middle(&state.cwd.display().to_string(), cwd_budget);
        left_width += 3 + display_width(&cwd);
        left.push(Span::styled(" / ", Style::default().fg(theme.border)));
        left.push(Span::styled(cwd, Style::default().fg(theme.muted)));
    }
    left.push(Span::raw(" ".repeat(
        width.saturating_sub(left_width + display_width(&security)),
    )));
    left.push(Span::styled(marker, Style::default().fg(color)));
    left.push(Span::styled(
        format!(" {}", report.label()),
        Style::default().fg(if report == ReportStatus::Verifying {
            theme.text
        } else {
            color
        }),
    ));
    frame.render_widget(Paragraph::new(Line::from(left)), inner);
}

pub(super) fn render_transcript(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    if state.show_startup() {
        render_startup(frame, state, theme, area);
        return;
    }
    let block = Block::default().padding(Padding::new(GUTTER, GUTTER, 1, 1));
    let inner = block.inner(area);
    let mut lines = state.transcript_lines(inner.width);
    let visible = usize::from(inner.height);
    // Short conversations sit against the composer rather than floating at the
    // top of an otherwise empty screen.
    if lines.len() < visible {
        let mut anchored = vec![Line::default(); visible - lines.len()];
        anchored.append(&mut lines);
        lines = anchored;
    }
    let max_scroll = u16::try_from(lines.len().saturating_sub(visible)).unwrap_or(u16::MAX);
    let scroll = if state.follow_output {
        max_scroll
    } else {
        state.scroll.min(max_scroll)
    };
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

pub(super) fn render_startup(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let width = area.width.saturating_sub(8).min(72);
    let height = area.height.saturating_sub(2).min(13);
    let panel = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    if panel.is_empty() {
        return;
    }
    let art: &[&str] = if !state.options.ascii && width >= 68 && height >= 11 {
        &brand::EAGLE
    } else if !state.options.ascii && width >= 50 && height >= 8 {
        &brand::COMPACT_EAGLE
    } else {
        &[]
    };
    let content = if art.is_empty() {
        panel
    } else {
        let art_width = u16::try_from(
            art.iter()
                .map(|line| display_width(line))
                .max()
                .unwrap_or(0),
        )
        .unwrap_or(32);
        let columns = Layout::horizontal([
            Constraint::Length(art_width),
            Constraint::Length(5),
            Constraint::Min(20),
        ])
        .split(panel);
        let mut lines = vec![
            Line::default();
            usize::from(
                columns[0]
                    .height
                    .saturating_sub(u16::try_from(art.len()).unwrap_or(0))
                    / 2
            )
        ];
        lines.extend(
            art.iter()
                .map(|line| Line::from(Span::styled(*line, Style::default().fg(theme.brand)))),
        );
        frame.render_widget(Paragraph::new(lines), columns[0]);
        columns[2]
    };
    let mut lines = vec![Line::default(); usize::from(content.height.saturating_sub(7) / 2)];
    lines.push(Line::from(vec![
        Span::styled(
            "AXIOM CLI",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  v{}", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme.muted),
        ),
    ]));
    if content.height >= 3 {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            state.splash_message,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )));
    }
    if content.height >= 7 {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Private AI. On your terms.",
            Style::default().fg(theme.muted),
        )));
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "/ commands   /theme appearance",
            Style::default().fg(theme.muted),
        )));
    }
    frame.render_widget(
        Paragraph::new(lines).alignment(if art.is_empty() {
            Alignment::Center
        } else {
            Alignment::Left
        }),
        content,
    );
}

pub(super) fn render_status(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(inner) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let status_color = if state.exit_armed {
        theme.warning
    } else {
        match state.status_tone {
            StatusTone::Neutral => theme.muted,
            StatusTone::Active => theme.brand,
            StatusTone::Waiting | StatusTone::Cancelled => theme.warning,
            StatusTone::Success => theme.success,
            StatusTone::Error => theme.error,
        }
    };
    let status_text = if state.exit_armed {
        "Press Ctrl+C again to exit".into()
    } else if state.search_active {
        format!(
            "/{}  ·  {} match(es)  ·  Enter accept  Esc cancel",
            state.search_query,
            state.search_matches.len()
        )
    } else if state.pending_attention.is_empty() {
        state.status.clone()
    } else {
        format!(
            "{} · {} pending",
            state.status,
            state.pending_attention.len()
        )
    };
    let status_text = if state.compacting && !state.exit_armed && !state.search_active {
        format!("{}  Compacting…", state.compaction_frame())
    } else {
        status_text
    };
    let marker = if state.exit_armed {
        state.glyph("!", "!")
    } else {
        state.glyph("●", "*")
    };
    let width = usize::from(inner.width);
    let fixed_width = display_width(marker).saturating_add(1);
    let status_text = truncate_end(&status_text, width.saturating_sub(fixed_width));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(marker, Style::default().fg(status_color)),
            Span::styled(format!(" {status_text}"), Style::default().fg(theme.muted)),
        ])),
        inner,
    );
}

pub(super) fn render_composer_controls(
    frame: &mut Frame<'_>,
    state: &TuiState,
    theme: Theme,
    area: Rect,
) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let details = state.model_details.get(&state.model);
    let model = details
        .map(|model| model.short_label.as_str())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(&state.model);
    let thinking = match state.thinking {
        ThinkingLevel::ProviderDefault => "Auto",
        ThinkingLevel::Enabled => "On",
        ThinkingLevel::Disabled => "Off",
        ThinkingLevel::Minimal => "Minimal",
        ThinkingLevel::Low => "Low",
        ThinkingLevel::Medium => "Medium",
        ThinkingLevel::High => "High",
        ThinkingLevel::ExtraHigh => "Extra high",
    };
    let permissions = match state.profile {
        PermissionProfile::None | PermissionProfile::Web => "Off",
        PermissionProfile::Observe if width < 48 => "Read",
        PermissionProfile::Observe => "Read only",
        PermissionProfile::Confirm => "Ask",
        PermissionProfile::FullAccess if width < 48 => "Full",
        PermissionProfile::FullAccess => "Full access",
    };
    let web = if state.web_enabled { "On" } else { "Off" };
    // Preserve the tool-access states at narrow widths, then give the model
    // the remaining space. Thinking is useful only for models that expose it.
    let permissions_label = format!("Permissions {permissions}");
    let web_label = format!("Web {web}");
    let thinking_label = format!("Thinking {thinking}");
    let fixed_width = display_width(&permissions_label) + display_width(&web_label) + 10;
    let show_thinking = details.map_or(state.thinking != ThinkingLevel::ProviderDefault, |model| {
        !crate::agent::supported_thinking_levels(model).is_empty()
    }) && width >= fixed_width + display_width(&thinking_label) + 15;
    let reserved = fixed_width
        + if show_thinking {
            display_width(&thinking_label) + 4
        } else {
            0
        };
    let model = truncate_middle(model, width.saturating_sub(reserved + 2).min(32));
    let control = Style::default();
    let label = control.fg(theme.muted);
    let value = control.fg(theme.text);
    let mut spans = vec![
        Span::styled(" ", control),
        Span::styled(model, value.add_modifier(Modifier::BOLD)),
        Span::styled(" ", control),
    ];
    if show_thinking {
        spans.extend([
            Span::raw("  "),
            Span::styled(" Thinking ", label),
            Span::styled(format!("{thinking} "), value),
        ]);
    }
    spans.extend([
        Span::raw("  "),
        Span::styled(" Permissions ", label),
        Span::styled(format!("{permissions} "), value),
        Span::raw("  "),
        Span::styled(
            format!(
                " {} ",
                state.glyph(
                    if state.web_enabled { "●" } else { "○" },
                    if state.web_enabled { "+" } else { "-" }
                )
            ),
            control.fg(if state.web_enabled {
                theme.brand
            } else {
                theme.muted
            }),
        ),
        Span::styled("Web ", label),
        Span::styled(web, if state.web_enabled { value } else { label }),
        Span::styled(" ", control),
    ]);
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.surface)),
        area,
    );
}

pub(super) fn render_composer(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(area) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let interacting = state.interaction.is_some();
    let focused = state.focus == Focus::Composer && !interacting;
    // A filled slab plus a single accent column reads as one control; a full box
    // border would put a second competing line next to the header divider.
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface)),
        area,
    );
    frame.render_widget(
        Block::default().style(Style::default().bg(if focused {
            theme.text
        } else {
            theme.border
        })),
        Rect {
            x: area.x,
            y: area.y,
            width: 1,
            height: area.height,
        },
    );

    if area.height >= 5 {
        render_composer_controls(
            frame,
            state,
            theme,
            Rect {
                x: area.x.saturating_add(2),
                y: area.bottom().saturating_sub(2),
                width: area.width.saturating_sub(4),
                height: 1,
            },
        );
    }

    let Some(inner) = composer_input_area(area) else {
        return;
    };
    let answering = state.answer_prompt.is_some() && !interacting;
    let marker = format!(
        "{} ",
        if answering {
            "?"
        } else {
            state.glyph("›", ">")
        }
    );
    frame.render_widget(
        Paragraph::new(marker).style(
            Style::default()
                .fg(if focused { theme.text } else { theme.muted })
                .bg(theme.surface),
        ),
        Rect {
            width: inner.width.min(2),
            ..inner
        },
    );

    let text_area = Rect {
        x: inner.x.saturating_add(2),
        width: inner.width.saturating_sub(2),
        ..inner
    };
    if text_area.is_empty() {
        return;
    }

    if interacting {
        frame.render_widget(
            Paragraph::new("Respond in the panel above")
                .style(Style::default().fg(theme.muted).bg(theme.surface)),
            text_area,
        );
        return;
    }

    if answering {
        let visible = tail_to_width(
            &state.answer_input,
            usize::from(text_area.width).saturating_sub(1),
        );
        let body = if visible.is_empty() && !focused {
            Span::styled("Tab to answer", Style::default().fg(theme.muted))
        } else {
            Span::styled(visible.clone(), Style::default().fg(theme.text))
        };
        frame.render_widget(
            Paragraph::new(body).style(Style::default().bg(theme.surface)),
            text_area,
        );
        if focused && state.overlay.is_none() && !state.search_active {
            let column = u16::try_from(display_width(&visible)).unwrap_or(u16::MAX);
            frame.set_cursor_position((
                text_area
                    .x
                    .saturating_add(column)
                    .min(text_area.right().saturating_sub(1)),
                text_area.y,
            ));
        }
        return;
    }

    let view = state.input.view(text_area.width, text_area.height);
    let body = if state.input.is_empty() {
        if focused {
            "Ask anything, or / for commands"
        } else {
            "Tab to type"
        }
        .to_owned()
    } else {
        view.lines.join("\n")
    };
    frame.render_widget(
        Paragraph::new(body).style(
            Style::default()
                .fg(if state.input.is_empty() {
                    theme.muted
                } else {
                    theme.text
                })
                .bg(theme.surface),
        ),
        text_area,
    );

    if focused && state.overlay.is_none() && !state.search_active {
        frame.set_cursor_position((
            text_area
                .x
                .saturating_add(view.cursor_column)
                .min(text_area.right().saturating_sub(1)),
            text_area
                .y
                .saturating_add(view.cursor_row)
                .min(text_area.bottom().saturating_sub(1)),
        ));
    }
}

/// The editable viewport inside the composer: a focus rail and padding,
/// with two extra rows reserved for controls when the terminal has space.
pub(super) fn composer_input_area(area: Rect) -> Option<Rect> {
    let width = area.width.checked_sub(3)?;
    (width > 0 && area.height >= 3).then(|| Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width,
        height: area
            .height
            .saturating_sub(if area.height >= 5 { 4 } else { 2 }),
    })
}

pub(super) fn render_hints(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(inner) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let separator = if state.options.ascii { " | " } else { " · " };
    let mut spans = Vec::new();
    let context = state.interaction.is_none().then(|| state.context_summary());
    let context_width = context.as_ref().map_or(0, |text| display_width(text) + 2);
    let hint_width = usize::from(inner.width).saturating_sub(context_width);
    let hints: Vec<(&str, &str)> = match &state.interaction {
        Some(InteractionView::Approval(request)) => {
            let mut hints = vec![("Ctrl+Y", "allow once")];
            if request.allow_session_grants {
                hints.push(("Ctrl+G", "exact/session"));
            }
            if request.allow_session_grants && request.suggested_prefix_scope.is_some() {
                hints.push(("Ctrl+P", "scope/session"));
            }
            hints.push(("Ctrl+N", "deny"));
            hints.push(("Esc", "stop task"));
            hints
        }
        Some(InteractionView::Questions { index, .. }) => {
            let mut hints = vec![("Enter", "submit answer")];
            if *index > 0 {
                hints.push(("Shift+Tab", "previous"));
            }
            hints.push(("Esc", "stop task"));
            hints
        }
        None if state.running => vec![
            ("Enter", "steer"),
            ("Esc", "stop"),
            ("Tab", "focus"),
            ("j/k", "scroll"),
            ("Ctrl+F", "search"),
            ("v", "view"),
            ("?", "help"),
        ],
        None if state.focus == Focus::Composer && state.input.starts_with('/') => vec![
            ("Enter", "run"),
            ("Tab", "complete"),
            ("j/k", "scroll"),
            ("Ctrl+F", "search"),
            ("v", "view"),
            ("?", "help"),
        ],
        None if state.focus == Focus::Transcript => vec![
            ("j/k", "select"),
            ("Enter", "details"),
            ("v", "view"),
            ("Ctrl+F", "search"),
            ("Tab", "compose"),
            ("?", "help"),
        ],
        None => vec![
            ("Enter", "send"),
            ("Alt+Enter", "newline"),
            ("Tab", "browse"),
            ("/", "commands"),
            ("?", "help"),
        ],
    };
    let mut used = 0;
    for (index, (key, label)) in hints.into_iter().enumerate() {
        let required = display_width(key)
            + 1
            + display_width(label)
            + if index > 0 {
                display_width(separator)
            } else {
                0
            };
        if used + required > hint_width {
            break;
        }
        used += required;
        if index > 0 {
            spans.push(Span::styled(separator, Style::default().fg(theme.border)));
        }
        spans.push(Span::styled(
            key,
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {label}"),
            Style::default().fg(theme.muted),
        ));
    }
    if let Some(context) = context {
        spans.push(Span::raw(" ".repeat(
            usize::from(inner.width).saturating_sub(used + display_width(&context)),
        )));
        spans.push(Span::styled(context, Style::default().fg(theme.muted)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

pub(super) fn render_interaction(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(interaction) = &state.interaction else {
        return;
    };
    let (title, body, editing) = match interaction {
        InteractionView::Approval(request) => {
            let mut choices = vec!["Ctrl+Y  Allow this operation once".to_owned()];
            if request.allow_session_grants {
                choices.push("Ctrl+G  Allow this exact operation for this session".into());
            }
            if let Some(scope) = request
                .suggested_prefix_scope
                .as_ref()
                .filter(|_| request.allow_session_grants)
            {
                choices.push(format!("Ctrl+P  Allow {scope} for this session"));
            }
            choices.push("Ctrl+N  Deny this operation".into());
            (
                " Permission required ",
                format!(
                    "The agent wants to perform:\n{}\n\nWhy approval is needed:\n{}\n\n{}",
                    describe_effect(&request.effect),
                    request.explanation,
                    choices.join("\n")
                ),
                false,
            )
        }
        InteractionView::Questions { request, index } => {
            let question = &request.questions[*index];
            let requirement = if question.required {
                "required"
            } else {
                "optional"
            };
            let options = question
                .options
                .iter()
                .enumerate()
                .map(|(option_index, option)| format!("  {}  {option}", option_index + 1))
                .collect::<Vec<_>>()
                .join("\n");
            let options = if options.is_empty() {
                String::new()
            } else {
                format!("\n\n{options}")
            };
            let optional = if question.required {
                ""
            } else {
                " Leave blank and press Enter to skip."
            };
            (
                " Axiom needs your input ",
                format!(
                    "Question {} of {} · {requirement}\n{}{}\n\nAnswer{}\n? {}",
                    index + 1,
                    request.questions.len(),
                    question.prompt,
                    options,
                    optional,
                    state.answer_input
                ),
                true,
            )
        }
    };
    let requested_rows = u16::try_from(body.lines().count())
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let area = fitted_rect(88, requested_rows.clamp(7, 18), frame.area());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(body.clone())
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(Span::styled(
                        title,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .padding(Padding::horizontal(1)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
    if editing && area.width > 6 && area.height > 2 {
        let visible = tail_to_width(
            &state.answer_input,
            usize::from(area.width.saturating_sub(6)),
        );
        let cursor_row = area
            .y
            .saturating_add(u16::try_from(body.lines().count()).unwrap_or(u16::MAX))
            .min(area.bottom().saturating_sub(2));
        frame.set_cursor_position((
            area.x
                .saturating_add(4)
                .saturating_add(u16::try_from(display_width(&visible)).unwrap_or(u16::MAX))
                .min(area.right().saturating_sub(2)),
            cursor_row,
        ));
    }
}

pub(super) fn render_overlay(
    frame: &mut Frame<'_>,
    state: &TuiState,
    overlay: &Overlay,
    theme: Theme,
) {
    if let Overlay::Security(view) = overlay {
        render_security_overlay(frame, state, *view, theme);
        return;
    }
    if let Overlay::Billing(view) = overlay {
        super::billing::render(frame, state, view, theme);
        return;
    }
    if matches!(overlay, Overlay::SaveAttestation(_)) {
        render_attestation_save_picker(frame, state, theme);
        return;
    }
    if let Overlay::Auth(auth) = overlay {
        render_auth_overlay(frame, state, auth, theme);
        return;
    }
    if matches!(overlay, Overlay::Permissions { .. }) {
        render_permissions_picker(frame, state, theme);
        return;
    }
    if matches!(overlay, Overlay::ModelPicker { .. }) {
        render_model_picker(frame, state, theme);
        return;
    }
    if matches!(overlay, Overlay::ResumePicker { .. }) {
        render_resume_picker(frame, state, theme);
        return;
    }
    if matches!(overlay, Overlay::DeletePicker { .. }) {
        render_delete_picker(frame, state, theme);
        return;
    }
    let (title, body): (&str, String) = match overlay {
        Overlay::Billing(_) => unreachable!("billing rendered separately"),
        Overlay::Usage => (" Usage · ↑/↓ scroll · Esc close ", state.usage_details()),
        Overlay::Help => (
            " Help ",
            "Tab: switch composer/transcript focus\n\
             Composer arrows/Home/End: move the editing cursor\n\
             Alt+Enter, Shift+Enter, or Ctrl+J: insert a new line\n\
             Enter: send, or show/hide selected task details\n\
             Transcript j/k or arrows: move through entries\n\
             Ctrl+F: search transcript; n/N: next/previous match\n\
             v: open selected entry in this terminal-viewport viewer\n\
             End: follow streaming output\n\
             /usage: inspect context capacity and compaction threshold\n\
             Ctrl+C: press twice to exit\n\
             Esc: stop active work, clear a composer draft, or close an overlay"
                .into(),
        ),
        Overlay::Entry(index) => {
            let body = state.entries.get(*index).map_or_else(
                || "Entry is no longer available".into(),
                |entry| {
                    let content = entry
                        .task
                        .as_ref()
                        .map_or_else(|| entry.text.clone(), TaskRunView::detail_text);
                    format!("entry {} · id {}\n\n{content}", index + 1, entry.id)
                },
            );
            (
                if state.running {
                    " Viewer · Esc to stop task "
                } else {
                    " Viewer · Esc to close "
                },
                body,
            )
        }
        Overlay::Permissions { .. } => unreachable!("permissions rendered separately"),
        Overlay::ModelPicker { .. } => unreachable!("model picker rendered separately"),
        Overlay::ResumePicker { .. } => unreachable!("resume picker rendered separately"),
        Overlay::DeletePicker { .. } => unreachable!("delete picker rendered separately"),
        Overlay::Security(_) => unreachable!("security inspector rendered separately"),
        Overlay::SaveAttestation(_) => {
            unreachable!("attestation save picker rendered separately")
        }
        Overlay::Auth(_) => unreachable!("auth overlay rendered separately"),
    };
    // Help is a fixed list, so give it only the rows it needs; the viewer holds
    // arbitrary entry text and keeps the tall panel.
    let area = match overlay {
        Overlay::Billing(_) => unreachable!("billing rendered separately"),
        Overlay::Help => {
            let rows = u16::try_from(body.lines().count()).unwrap_or(u16::MAX);
            fitted_rect(88, rows.saturating_add(2), frame.area())
        }
        Overlay::Entry(_) | Overlay::Usage => centered_rect(88, 82, frame.area()),
        Overlay::Permissions { .. } => unreachable!("permissions rendered separately"),
        Overlay::ModelPicker { .. } => unreachable!("model picker rendered separately"),
        Overlay::ResumePicker { .. } => unreachable!("resume picker rendered separately"),
        Overlay::DeletePicker { .. } => unreachable!("delete picker rendered separately"),
        Overlay::Security(_) => unreachable!("security inspector rendered separately"),
        Overlay::SaveAttestation(_) => {
            unreachable!("attestation save picker rendered separately")
        }
        Overlay::Auth(_) => unreachable!("auth overlay rendered separately"),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(Span::styled(
                        title,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .padding(Padding::horizontal(1)),
            )
            .scroll((state.overlay_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn render_permissions_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(Overlay::Permissions { selected }) = &state.overlay else {
        return;
    };
    let area = fitted_rect(82, 23, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Permissions ",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().fg(theme.text).bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height < 3 {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "Tools",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        rows[0],
    );
    let selected = (*selected).min(PermissionProfile::ALL.len() - 1);
    let mut lines = Vec::new();
    for (index, profile) in PermissionProfile::ALL.into_iter().enumerate() {
        let is_selected = index == selected;
        let marker = if is_selected {
            state.glyph("›", ">")
        } else {
            " "
        };
        let is_current = profile == state.profile;
        let current = if is_current {
            state.glyph("✓", "+")
        } else {
            " "
        };
        let style = if is_selected {
            Style::default()
                .fg(theme.text)
                .patch(theme.selection())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let current_style = if is_current {
            style.fg(theme.success)
        } else {
            style
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker} "), style),
            Span::styled(format!("{current} "), current_style),
            Span::styled(
                truncate_end(
                    profile.label(),
                    usize::from(rows[1].width).saturating_sub(4),
                ),
                style,
            ),
        ]));
        lines.push(Line::from(Span::styled(
            truncate_end(
                &format!("    {}", profile.description()),
                usize::from(rows[1].width),
            ),
            Style::default().fg(theme.muted),
        )));
        if index + 1 < PermissionProfile::ALL.len() {
            lines.push(Line::from(""));
        }
    }
    let visible_height = usize::from(rows[1].height);
    let scroll = if lines.len() > visible_height {
        selected
            .saturating_mul(3)
            .min(lines.len().saturating_sub(visible_height))
    } else {
        0
    };
    let scroll = u16::try_from(scroll).unwrap_or(u16::MAX);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_end(
                "↑/↓ select · Enter apply · Esc cancel",
                usize::from(rows[2].width),
            ),
            Style::default().fg(theme.muted),
        )),
        rows[2],
    );
}

pub(super) fn render_attestation_save_picker(
    frame: &mut Frame<'_>,
    state: &TuiState,
    theme: Theme,
) {
    let Some(Overlay::SaveAttestation(picker)) = &state.overlay else {
        return;
    };
    let area = centered_rect(82, 72, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Save attestation ",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::horizontal(1))
        .style(Style::default().fg(theme.text).bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height < 7 {
        return;
    }

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(inner);
    let directory_text = truncate_middle(
        &picker.directory.display().to_string(),
        usize::from(rows[0].width).saturating_sub(display_width("Folder  ")),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Folder  ",
                Style::default().fg(if picker.focus == SavePickerFocus::Directories {
                    theme.text
                } else {
                    theme.muted
                }),
            ),
            Span::styled(directory_text, Style::default().fg(theme.text)),
        ])),
        rows[0],
    );

    let filename_width = usize::from(rows[1].width).saturating_sub(display_width("File    "));
    let visible_filename = tail_to_width(&picker.filename, filename_width);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "File    ",
                Style::default().fg(if picker.focus == SavePickerFocus::Filename {
                    theme.text
                } else {
                    theme.muted
                }),
            ),
            Span::styled(visible_filename.clone(), Style::default().fg(theme.text)),
        ])),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Span::styled(
            "Folders",
            Style::default()
                .fg(theme.muted)
                .add_modifier(Modifier::BOLD),
        )),
        rows[2],
    );
    let directories = attestation_picker_directories(&picker.directory);
    let visible_height = usize::from(rows[3].height).max(1);
    let selected = picker.selected.min(directories.len().saturating_sub(1));
    let start = selected
        .saturating_add(1)
        .saturating_sub(visible_height)
        .min(directories.len().saturating_sub(visible_height));
    let directory_lines = if directories.is_empty() {
        vec![Line::from(Span::styled(
            "  No folders available",
            Style::default().fg(theme.muted),
        ))]
    } else {
        directories
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_height)
            .map(|(index, (label, _))| {
                let is_selected = picker.focus == SavePickerFocus::Directories && index == selected;
                let marker = if is_selected {
                    state.glyph("›", ">")
                } else {
                    " "
                };
                let style = if is_selected {
                    Style::default()
                        .fg(theme.text)
                        .patch(theme.selection())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text)
                };
                Line::from(Span::styled(
                    truncate_end(&format!("{marker} {label}"), usize::from(rows[3].width)),
                    style,
                ))
            })
            .collect()
    };
    frame.render_widget(Paragraph::new(directory_lines), rows[3]);

    frame.render_widget(
        Paragraph::new(Span::styled(
            picker.error.as_deref().unwrap_or(""),
            Style::default().fg(theme.error),
        )),
        rows[4],
    );
    let help = match picker.focus {
        SavePickerFocus::Filename => {
            "Type filename · Ctrl+U clear · Enter save · Tab browse folders · Esc back"
        }
        SavePickerFocus::Directories => {
            "↑/↓ select · Enter open · ← parent · S save · Tab filename · Esc back"
        }
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_end(help, usize::from(rows[5].width)),
            Style::default().fg(theme.muted),
        )),
        rows[5],
    );

    if picker.focus == SavePickerFocus::Filename {
        let cursor_x = rows[1]
            .x
            .saturating_add(u16::try_from(display_width("File    ")).unwrap_or(u16::MAX))
            .saturating_add(u16::try_from(display_width(&visible_filename)).unwrap_or(u16::MAX))
            .min(rows[1].right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, rows[1].y));
    }
}

pub(super) fn render_security_overlay(
    frame: &mut Frame<'_>,
    state: &TuiState,
    view: SecurityView,
    theme: Theme,
) {
    let area = centered_rect(88, 86, frame.area());
    frame.render_widget(Clear, area);
    let title = match view {
        SecurityView::Summary => " Security ",
        SecurityView::Workload => " Provider evidence ",
        SecurityView::Raw => " Raw verification report ",
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::horizontal(1))
        .style(Style::default().fg(theme.text).bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 2 || inner.width == 0 {
        return;
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let lines = security_overlay_lines(state, view, theme);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .scroll((state.overlay_scroll, 0))
            .wrap(Wrap { trim: false }),
        rows[0],
    );
    let mut help = vec![
        Span::styled("V", Style::default().fg(theme.text)),
        Span::styled(" next view  ·  ", Style::default().fg(theme.muted)),
        Span::styled("R", Style::default().fg(theme.text)),
        Span::styled(" verify again  ·  ", Style::default().fg(theme.muted)),
    ];
    if view == SecurityView::Raw && state.security_evidence.is_some() {
        help.extend([
            Span::styled("S", Style::default().fg(theme.text)),
            Span::styled(" save  ·  ", Style::default().fg(theme.muted)),
        ]);
    }
    help.push(Span::styled(
        "↑/↓ scroll  ·  Esc close",
        Style::default().fg(theme.muted),
    ));
    frame.render_widget(Paragraph::new(Line::from(help)), rows[1]);
}

pub(super) fn security_overlay_lines(
    state: &TuiState,
    view: SecurityView,
    theme: Theme,
) -> Vec<Line<'static>> {
    if state.security == "VERIFYING" {
        return vec![
            Line::from(vec![
                Span::styled(
                    format!("{}  ", state.activity_glyph(false)),
                    Style::default().fg(state.activity_color(theme, 0)),
                ),
                Span::styled(
                    "Verifying fresh hardware evidence and endpoint bindings…",
                    Style::default().fg(theme.text),
                ),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "No prompt will be sent until verification succeeds.",
                Style::default().fg(theme.muted),
            )),
        ];
    }

    let Some(evidence) = &state.security_evidence else {
        let message = if state.security == "SECURITY FAILED" {
            "The latest security verification failed. Press R to try again."
        } else {
            "No attestation evidence is available. Press R to verify now."
        };
        let mut lines = vec![Line::from(Span::styled(
            message,
            Style::default().fg(if state.security == "SECURITY FAILED" {
                theme.error
            } else {
                theme.warning
            }),
        ))];
        lines.extend(security_failure_lines(state, theme));
        return lines;
    };

    let report = state.report_status();
    let mut lines = vec![
        Line::from(Span::styled(
            match report {
                ReportStatus::Verified => {
                    "Current TEE report verified locally. Reply completion is verified separately."
                }
                ReportStatus::Expired => {
                    "This report has expired. Renewal resumes when active and available; press R to retry."
                }
                ReportStatus::Idle => {
                    "Idle after 30 minutes without activity. Verification resumes when you return."
                }
                ReportStatus::Failed => {
                    "Verification failed. This retained report is not current proof."
                }
                ReportStatus::Degraded => {
                    "TEE updates needed. Accepted for this provider until restart; other checks still apply."
                }
                ReportStatus::Outdated => {
                    "TEE updates needed. To continue: /security accept-outdated (until restart)."
                }
                _ => "Current proof is unavailable. This is a previous report; press R to verify.",
            },
            Style::default().fg(
                if matches!(report, ReportStatus::Verified | ReportStatus::Idle) {
                    theme.muted
                } else {
                    theme.warning
                },
            ),
        )),
        Line::default(),
    ];
    lines.extend(security_failure_lines(state, theme));
    lines.extend(match view {
        SecurityView::Summary => security_summary_lines(evidence, report, theme),
        SecurityView::Workload => workload_lines(evidence, false, theme),
        SecurityView::Raw => workload_lines(evidence, true, theme),
    });
    lines
}

fn security_failure_lines(state: &TuiState, theme: Theme) -> Vec<Line<'static>> {
    let Some(detail) = state
        .security_error
        .as_deref()
        .filter(|_| state.security == "SECURITY FAILED")
    else {
        return Vec::new();
    };
    let mut lines = vec![Line::default(), Line::from("Failure reason:")];
    lines.extend(detail.lines().map(|line| {
        Line::from(Span::styled(
            line.to_owned(),
            Style::default().fg(theme.error),
        ))
    }));
    lines
}

pub(super) fn security_summary_lines(
    evidence: &SecurityEvidence,
    report: ReportStatus,
    theme: Theme,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    security_field(
        &mut lines,
        "Status",
        report.label(),
        if report == ReportStatus::Verified {
            theme.success
        } else {
            theme.warning
        },
        theme,
    );
    security_field(&mut lines, "Model", &evidence.model_id, theme.text, theme);
    security_field(
        &mut lines,
        "Provider",
        &evidence.provider_id,
        theme.text,
        theme,
    );
    security_field(
        &mut lines,
        "Checked",
        &format_verification_time(evidence.verified_at_unix_seconds),
        theme.text,
        theme,
    );
    security_field(
        &mut lines,
        "Expires",
        &evidence
            .hard_expires_at_unix_seconds
            .map_or_else(|| "Unavailable".into(), format_verification_time),
        theme.text,
        theme,
    );
    if let Some(generation) = evidence.attestation_generation {
        security_field(
            &mut lines,
            "Generation",
            &generation.to_string(),
            theme.text,
            theme,
        );
    }
    security_field(
        &mut lines,
        "Attestation",
        &evidence.attestation_protocol,
        theme.text,
        theme,
    );
    security_field(
        &mut lines,
        "Encryption",
        &format!(
            "{} v{}",
            evidence.e2ee_protocol, evidence.e2ee_encryption_version
        ),
        theme.text,
        theme,
    );
    security_field(
        &mut lines,
        "Trust policy",
        &evidence.trust_policy_version,
        theme.text,
        theme,
    );

    lines.push(Line::from(""));
    lines.push(section_line("Verification checks", theme));
    for check in &evidence.checks {
        lines.push(Line::from(vec![
            Span::styled(
                if check.passed { "✓ " } else { "× " },
                Style::default().fg(if check.passed {
                    theme.success
                } else if report == ReportStatus::Degraded
                    && check.id == "intel_tdx"
                    && check.status == "OutOfDate"
                {
                    theme.warning
                } else {
                    theme.error
                }),
            ),
            Span::styled(
                sanitize_terminal_text(&check.label),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("  {}", sanitize_terminal_text(&check.status)),
                Style::default().fg(theme.muted),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(section_line("Key bindings", theme));
    security_field(
        &mut lines,
        "Encryption key",
        &evidence.model_key_fingerprint,
        theme.text,
        theme,
    );
    security_field(
        &mut lines,
        "TLS identity",
        evidence
            .tls_spki_fingerprint
            .as_deref()
            .unwrap_or("Unavailable"),
        theme.text,
        theme,
    );
    lines.push(Line::default());
    lines.extend(provider_claim_lines(evidence, theme));
    lines
}

fn provider_claim_lines(evidence: &SecurityEvidence, theme: Theme) -> Vec<Line<'static>> {
    let mut lines = vec![
        section_line("Provider evidence fields", theme),
        Line::from(
            "Local check results identify what was verified; field names and formats are provider-specific.",
        ),
    ];
    for claim in &evidence.provider_claims {
        security_field(
            &mut lines,
            &humanize_evidence_key(&claim.name),
            &claim.value,
            theme.text,
            theme,
        );
    }
    if evidence.provider_claims.is_empty() {
        lines.push(Line::from("No additional fields were supplied."));
    }
    lines
}

pub(super) fn workload_lines(
    evidence: &SecurityEvidence,
    raw: bool,
    theme: Theme,
) -> Vec<Line<'static>> {
    let mut lines = if raw {
        Vec::new()
    } else {
        provider_claim_lines(evidence, theme)
    };
    let (heading, content) = if raw {
        (
            "Retained verification report (JSON)",
            serde_json::to_string_pretty(evidence)
                .unwrap_or_else(|_| "Report could not be encoded".into()),
        )
    } else if let Some(document) = &evidence.workload_manifest {
        ("Retained workload evidence", document.clone())
    } else {
        lines.push(Line::default());
        lines.push(Line::from("This provider has no separate workload document. Inspect its checks and evidence fields, or save the full report."));
        return lines;
    };
    let (content, truncated) = bounded_workload_display(&sanitize_terminal_text(&content));
    lines.extend([
        Line::default(),
        section_line(heading, theme),
        Line::from(Span::styled(
            "Public evidence; terminal display is sanitized. Save exports the full original report.",
            Style::default().fg(theme.muted),
        )),
        Line::from(""),
    ]);
    lines.extend(content.lines().map(|line| Line::from(line.to_owned())));
    if truncated {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Display limited to {MAX_WORKLOAD_DISPLAY_CHARS} characters."),
            Style::default().fg(theme.warning),
        )));
    }
    lines
}

pub(super) fn security_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    value_color: Color,
    theme: Theme,
) {
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<20}  ", sanitize_terminal_text(label)),
            Style::default().fg(theme.muted),
        ),
        Span::styled(
            sanitize_terminal_text(value),
            Style::default().fg(value_color),
        ),
    ]));
}

pub(super) fn section_line(label: &str, theme: Theme) -> Line<'static> {
    Line::from(Span::styled(
        label.to_owned(),
        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn format_verification_time(timestamp: u64) -> String {
    i64::try_from(timestamp)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map_or_else(
            || format!("Unix timestamp {timestamp}"),
            |time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        )
}

pub(super) fn render_auth_overlay(
    frame: &mut Frame<'_>,
    state: &TuiState,
    overlay: &AuthOverlay,
    theme: Theme,
) {
    let height = match overlay {
        AuthOverlay::Menu { message, .. } => {
            if message.is_some() {
                12
            } else {
                10
            }
        }
        AuthOverlay::Starting => 7,
        AuthOverlay::Browser { .. } => 15,
        AuthOverlay::Checking { .. } => 5,
        AuthOverlay::Account { status } => u16::try_from(status.lines().count())
            .unwrap_or(u16::MAX)
            .saturating_add(6),
    };
    let area = fitted_rect(78, height, frame.area());
    frame.render_widget(Clear, area);
    let title = match overlay {
        AuthOverlay::Menu { .. } => " Connect to Axiom ",
        AuthOverlay::Account { .. } => " Axiom account ",
        AuthOverlay::Starting | AuthOverlay::Browser { .. } => " Browser sign-in ",
        AuthOverlay::Checking { .. } => " Checking authorization ",
    };
    let mut lines = Vec::new();
    match overlay {
        AuthOverlay::Menu { message } => {
            lines.push(Line::from(Span::styled(
                "Sign in to start using AxiomCLI.",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )));
            if let Some(message) = message {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    sanitize_terminal_text(message),
                    Style::default().fg(theme.warning),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(" › ", Style::default().fg(theme.text)),
                Span::styled(
                    "Continue in system browser",
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter continue · Esc close",
                Style::default().fg(theme.muted),
            )));
        }
        AuthOverlay::Starting => {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}  ", state.activity_glyph(false)),
                    Style::default().fg(state.activity_color(theme, 0)),
                ),
                Span::styled(
                    "Starting a short-lived authorization…",
                    Style::default().fg(theme.text),
                ),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Esc cancels",
                Style::default().fg(theme.muted),
            )));
        }
        AuthOverlay::Browser {
            user_code,
            authorization_url,
            browser_opened,
        } => {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}  ", state.activity_glyph(false)),
                    Style::default().fg(state.activity_color(theme, 0)),
                ),
                Span::styled(
                    "Waiting for approval in your browser",
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Confirm that this code matches the website:",
                Style::default().fg(theme.muted),
            )));
            lines.push(Line::from(Span::styled(
                format!("  {user_code}"),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                if *browser_opened {
                    "The approval page was opened automatically."
                } else {
                    "The browser did not open automatically. Press O or open:"
                },
                Style::default().fg(theme.muted),
            )));
            lines.push(Line::from(Span::styled(
                authorization_url,
                Style::default()
                    .fg(theme.text)
                    .add_modifier(Modifier::UNDERLINED),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "O reopen browser · Esc cancel",
                Style::default().fg(theme.muted),
            )));
        }
        AuthOverlay::Checking { label } => {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}  ", state.activity_glyph(false)),
                    Style::default().fg(state.activity_color(theme, 0)),
                ),
                Span::styled(*label, Style::default().fg(theme.text)),
            ]));
        }
        AuthOverlay::Account { status } => {
            for line in sanitize_terminal_text(status).lines() {
                lines.push(Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(theme.text),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter or Esc to close",
                Style::default().fg(theme.muted),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(Span::styled(
                        title,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .padding(Padding::new(2, 2, 1, 1)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(super) fn render_model_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(Overlay::ModelPicker { query, selected }) = &state.overlay else {
        return;
    };
    let rows = u16::try_from(state.model_candidates.len())
        .unwrap_or(10)
        .clamp(3, 10);
    let area = fitted_rect(76, rows + 6, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Select model ",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::horizontal(1))
        .style(Style::default().fg(theme.text).bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height < 4 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let search_width = usize::from(rows[0].width).saturating_sub(display_width("Search  "));
    let visible_query = tail_to_width(query, search_width);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Search  ",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(visible_query.clone(), Style::default().fg(theme.text)),
        ])),
        rows[0],
    );

    let matches = state.model_picker_match_indices();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("Current  ", Style::default().fg(theme.muted)),
            Span::styled(state.model.clone(), Style::default().fg(theme.text)),
            Span::styled(
                format!("  ·  {} match(es)", matches.len()),
                Style::default().fg(theme.muted),
            ),
        ])),
        rows[1],
    );

    let list_height = usize::from(rows[2].height);
    let mut lines = Vec::new();
    if state.model_catalog_loading {
        lines.push(Line::from(Span::styled(
            "· Loading available models…",
            Style::default().fg(theme.muted),
        )));
    } else if state.model_candidates.is_empty() {
        lines.push(Line::from(Span::styled(
            if state.model_catalog_loaded {
                "No available models were returned by the provider."
            } else {
                "The model catalog could not be loaded. Close and run /model to retry."
            },
            Style::default().fg(theme.warning),
        )));
    } else if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "No models match this search.",
            Style::default().fg(theme.muted),
        )));
    } else {
        let selected = (*selected).min(matches.len() - 1);
        let start = selected.saturating_add(1).saturating_sub(list_height);
        let end = (start + list_height).min(matches.len());
        for (match_position, model_index) in matches[start..end].iter().enumerate() {
            let match_position = start + match_position;
            let model = &state.model_candidates[*model_index];
            let is_selected = match_position == selected;
            let is_current = model == &state.model;
            let marker = if is_selected {
                state.glyph("›", ">")
            } else {
                " "
            };
            let current = if is_current {
                state.glyph(" ✓", " *")
            } else {
                ""
            };
            let row_width = usize::from(rows[2].width);
            let model_label = state
                .model_details
                .get(model)
                .map_or(model.as_str(), |details| details.short_label.as_str());
            let left = format!("{marker} {model_label}{current}");
            let pricing = state
                .model_details
                .get(model)
                .and_then(model_pricing_summary);
            let label = pricing.map_or_else(
                || truncate_end(&left, row_width),
                |pricing| {
                    let pricing_width = display_width(&pricing);
                    if pricing_width.saturating_add(4) >= row_width {
                        truncate_end(&left, row_width)
                    } else {
                        let left = truncate_end(
                            &left,
                            row_width.saturating_sub(pricing_width).saturating_sub(2),
                        );
                        let padding = row_width
                            .saturating_sub(display_width(&left))
                            .saturating_sub(pricing_width);
                        format!("{left}{}{pricing}", " ".repeat(padding))
                    }
                },
            );
            let style = if is_selected {
                Style::default()
                    .fg(theme.text)
                    .patch(theme.selection())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
    }
    frame.render_widget(Paragraph::new(lines), rows[2]);
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_end(
                "Type to search · ↑/↓ to scroll · Enter select · Esc cancel",
                usize::from(rows[3].width),
            ),
            Style::default().fg(theme.muted),
        )),
        rows[3],
    );

    let cursor_x = rows[0]
        .x
        .saturating_add(u16::try_from(display_width("Search  ")).unwrap_or(u16::MAX))
        .saturating_add(u16::try_from(display_width(&visible_query)).unwrap_or(u16::MAX))
        .min(rows[0].right().saturating_sub(1));
    frame.set_cursor_position((cursor_x, rows[0].y));
}

pub(super) fn format_price_microusd(microusd: u64) -> String {
    let dollars = microusd / 1_000_000;
    let fraction = microusd % 1_000_000;
    if fraction == 0 {
        return format!("${dollars}");
    }
    let fraction = format!("{fraction:06}").trim_end_matches('0').to_owned();
    format!("${dollars}.{fraction}")
}

pub(super) fn model_pricing_summary(model: &axiom_inference::ModelInfo) -> Option<String> {
    let input = model.input_price_microusd_per_million_tokens?;
    let output = model.output_price_microusd_per_million_tokens?;
    Some(format!(
        "{}/{} · 1M",
        format_price_microusd(input),
        format_price_microusd(output)
    ))
}

pub(super) fn render_resume_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(Overlay::ResumePicker {
        query,
        selected,
        sessions,
    }) = &state.overlay
    else {
        return;
    };
    let area = centered_rect(82, 78, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Resume transcript ",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::horizontal(1))
        .style(Style::default().fg(theme.text).bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height < 4 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let search_width = usize::from(rows[0].width).saturating_sub(display_width("Search  "));
    let visible_query = tail_to_width(query, search_width);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Search  ",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(visible_query.clone(), Style::default().fg(theme.text)),
        ])),
        rows[0],
    );

    let matches = state.resume_picker_match_indices();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} saved", sessions.len()),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("  ·  {} match(es)  ·  this workspace", matches.len()),
                Style::default().fg(theme.muted),
            ),
        ])),
        rows[1],
    );

    let list_height = usize::from(rows[2].height);
    let mut lines = Vec::new();
    if sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No earlier transcripts are saved for this workspace.",
            Style::default().fg(theme.muted),
        )));
    } else if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "No transcripts match this search.",
            Style::default().fg(theme.muted),
        )));
    } else {
        let selected = (*selected).min(matches.len() - 1);
        let start = selected.saturating_add(1).saturating_sub(list_height);
        let end = (start + list_height).min(matches.len());
        for (match_position, session_index) in matches[start..end].iter().enumerate() {
            let match_position = start + match_position;
            let session = &sessions[*session_index];
            let is_selected = match_position == selected;
            let marker = if is_selected {
                state.glyph("›", ">")
            } else {
                " "
            };
            let title =
                sanitize_terminal_text(session.title.as_deref().unwrap_or("Untitled transcript"));
            let updated = session.updated_at.get(..16).unwrap_or(&session.updated_at);
            let updated = updated.replace('T', " ");
            let short_id = session.id.get(..8).unwrap_or(&session.id);
            let label = truncate_end(
                &format!("{marker} {title}  ·  {updated}  ·  {short_id}"),
                usize::from(rows[2].width),
            );
            let style = if is_selected {
                Style::default()
                    .fg(theme.text)
                    .patch(theme.selection())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
    }
    frame.render_widget(Paragraph::new(lines), rows[2]);
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_end(
                "Type to search · ↑/↓ to scroll · Enter resume · Esc cancel",
                usize::from(rows[3].width),
            ),
            Style::default().fg(theme.muted),
        )),
        rows[3],
    );

    let cursor_x = rows[0]
        .x
        .saturating_add(u16::try_from(display_width("Search  ")).unwrap_or(u16::MAX))
        .saturating_add(u16::try_from(display_width(&visible_query)).unwrap_or(u16::MAX))
        .min(rows[0].right().saturating_sub(1));
    frame.set_cursor_position((cursor_x, rows[0].y));
}

pub(super) fn render_delete_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(Overlay::DeletePicker {
        query,
        selected,
        sessions,
        marked,
        confirming,
    }) = &state.overlay
    else {
        return;
    };
    let area = centered_rect(88, 82, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Delete transcripts ",
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if *confirming {
            theme.error
        } else {
            theme.border
        }))
        .padding(Padding::horizontal(1))
        .style(Style::default().fg(theme.text).bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height < 5 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    let search_width = usize::from(rows[0].width).saturating_sub(display_width("Search  "));
    let visible_query = tail_to_width(query, search_width);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Search  ",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(visible_query.clone(), Style::default().fg(theme.text)),
        ])),
        rows[0],
    );

    let matches = state.delete_picker_match_indices();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!("{} saved", sessions.len()),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!(
                    "  ·  {} match(es)  ·  {} selected",
                    matches.len(),
                    marked.len()
                ),
                Style::default().fg(theme.muted),
            ),
        ])),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(Span::styled(
            if *confirming {
                format!(
                    "Permanently delete {} transcript(s)? This cannot be undone.  y confirm · n cancel",
                    marked.len()
                )
            } else {
                "Only explicitly checked transcripts will be removed.".into()
            },
            Style::default().fg(if *confirming { theme.error } else { theme.muted }),
        )),
        rows[2],
    );

    let list_height = usize::from(rows[3].height);
    let mut lines = Vec::new();
    if sessions.is_empty() {
        lines.push(Line::from(Span::styled(
            "No other saved transcripts are available.",
            Style::default().fg(theme.muted),
        )));
    } else if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "No transcripts match this search.",
            Style::default().fg(theme.muted),
        )));
    } else {
        let selected = (*selected).min(matches.len() - 1);
        let start = selected.saturating_add(1).saturating_sub(list_height);
        let end = (start + list_height).min(matches.len());
        for (match_position, session_index) in matches[start..end].iter().enumerate() {
            let match_position = start + match_position;
            let session = &sessions[*session_index];
            let is_selected = match_position == selected;
            let cursor = if is_selected {
                state.glyph("›", ">")
            } else {
                " "
            };
            let check = if marked.contains(&session.id) {
                "[x]"
            } else {
                "[ ]"
            };
            let title =
                sanitize_terminal_text(session.title.as_deref().unwrap_or("Untitled transcript"));
            let short_id = session.id.get(..8).unwrap_or(&session.id);
            let workspace = session
                .cwd
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| session.cwd.to_str().unwrap_or("workspace"));
            let archived = if session.archived { " · archived" } else { "" };
            let label = truncate_end(
                &format!("{cursor} {check} {title}  ·  {workspace}  ·  {short_id}{archived}"),
                usize::from(rows[3].width),
            );
            let style = if is_selected {
                Style::default()
                    .fg(theme.text)
                    .patch(theme.selection())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
    }
    frame.render_widget(Paragraph::new(lines), rows[3]);
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_end(
                if *confirming {
                    "y permanently delete · n/Esc keep selection"
                } else {
                    "Type search · Space toggle · Ctrl+A select visible · Enter continue · Esc cancel"
                },
                usize::from(rows[4].width),
            ),
            Style::default().fg(theme.muted),
        )),
        rows[4],
    );

    if !*confirming {
        let cursor_x = rows[0]
            .x
            .saturating_add(u16::try_from(display_width("Search  ")).unwrap_or(u16::MAX))
            .saturating_add(u16::try_from(display_width(&visible_query)).unwrap_or(u16::MAX))
            .min(rows[0].right().saturating_sub(1));
        frame.set_cursor_position((cursor_x, rows[0].y));
    }
}
