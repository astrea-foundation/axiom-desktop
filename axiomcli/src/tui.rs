use std::{collections::BTreeMap, env, fmt::Write as _, io, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEventKind,
        KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt as _;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    AxiomError, Result,
    agent::{APP_EVENT_QUEUE_CAPACITY, QuestionHandler, TurnContext, TurnRunner},
    app::{
        AppCommand, AppEvent, Origin, PermissionProfile, QuestionRequest, Runtime, SessionId,
        TurnId,
    },
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::SessionStore,
};

/// Every region of the shell is inset by the same number of columns so text,
/// dividers and the composer all line up on one left edge.
const GUTTER: u16 = 2;
/// Columns reserved for the role marker; entry bodies are indented to match so
/// wrapped continuations stay under the first line instead of falling to zero.
const BODY_INDENT: usize = 2;
/// Smallest terminal the full layout is designed for.
const MIN_WIDTH: u16 = 42;
const MIN_HEIGHT: u16 = 10;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorMode {
    #[default]
    TrueColor,
    Ansi256,
    Ansi16,
    NoColor,
}

impl ColorMode {
    fn detect() -> Self {
        Self::detect_from(
            env::var("AXIOMCLI_COLOR").ok().as_deref(),
            env::var_os("NO_COLOR").is_some(),
            env::var("TERM").ok().as_deref(),
            env::var("COLORTERM").ok().as_deref(),
        )
    }

    fn detect_from(
        explicit: Option<&str>,
        no_color: bool,
        term: Option<&str>,
        color_term: Option<&str>,
    ) -> Self {
        match explicit {
            Some("none" | "never") => return Self::NoColor,
            Some("16") => return Self::Ansi16,
            Some("256") => return Self::Ansi256,
            Some("truecolor" | "24bit") => return Self::TrueColor,
            Some(_) | None => {}
        }
        if no_color || term == Some("dumb") {
            Self::NoColor
        } else if color_term
            .is_some_and(|value| value.contains("truecolor") || value.contains("24bit"))
        {
            Self::TrueColor
        } else if term.is_some_and(|value| value.contains("256color")) {
            Self::Ansi256
        } else {
            Self::Ansi16
        }
    }
}

/// Three background tones give the shell its depth: `base` is the slab the whole
/// frame is painted with, `surface` lifts the composer and overlays off it, and
/// `border` is the low-contrast tone used for dividers so structure never has to
/// borrow the cherry accent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Theme {
    cherry: Color,
    cherry_bright: Color,
    cherry_deep: Color,
    base: Color,
    surface: Color,
    border: Color,
    on_accent: Color,
    text: Color,
    muted: Color,
    success: Color,
    warning: Color,
    error: Color,
}

impl Theme {
    const fn for_mode(mode: ColorMode) -> Self {
        match mode {
            ColorMode::TrueColor => Self {
                cherry: Color::Rgb(255, 77, 109),
                cherry_bright: Color::Rgb(255, 138, 158),
                cherry_deep: Color::Rgb(94, 26, 40),
                base: Color::Rgb(16, 16, 19),
                surface: Color::Rgb(29, 29, 34),
                border: Color::Rgb(46, 46, 53),
                on_accent: Color::Rgb(16, 16, 19),
                text: Color::Rgb(238, 238, 242),
                muted: Color::Rgb(124, 124, 136),
                success: Color::Rgb(126, 226, 168),
                warning: Color::Rgb(235, 181, 92),
                error: Color::Rgb(255, 95, 95),
            },
            ColorMode::Ansi256 => Self {
                cherry: Color::Indexed(204),
                cherry_bright: Color::Indexed(211),
                cherry_deep: Color::Indexed(52),
                base: Color::Indexed(233),
                surface: Color::Indexed(236),
                border: Color::Indexed(239),
                on_accent: Color::Indexed(233),
                text: Color::Indexed(253),
                muted: Color::Indexed(245),
                success: Color::Indexed(114),
                warning: Color::Indexed(179),
                error: Color::Indexed(203),
            },
            ColorMode::Ansi16 => Self {
                cherry: Color::LightMagenta,
                cherry_bright: Color::LightRed,
                cherry_deep: Color::Magenta,
                base: Color::Black,
                surface: Color::Black,
                border: Color::DarkGray,
                on_accent: Color::Black,
                text: Color::White,
                muted: Color::DarkGray,
                success: Color::LightGreen,
                warning: Color::Yellow,
                error: Color::LightRed,
            },
            ColorMode::NoColor => Self {
                cherry: Color::Reset,
                cherry_bright: Color::Reset,
                cherry_deep: Color::Reset,
                base: Color::Reset,
                surface: Color::Reset,
                border: Color::Reset,
                on_accent: Color::Reset,
                text: Color::Reset,
                muted: Color::Reset,
                success: Color::Reset,
                warning: Color::Reset,
                error: Color::Reset,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Composer,
    Transcript,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Overlay {
    Entry(usize),
    Help,
}

#[derive(Clone, Copy, Debug)]
pub struct TuiOptions {
    pub color_mode: ColorMode,
    pub ascii: bool,
    pub animation: bool,
}

impl TuiOptions {
    #[must_use]
    pub fn from_environment(animation: bool) -> Self {
        let ascii = env::var("AXIOMCLI_ASCII").as_deref() == Ok("1")
            || env::var("TERM").as_deref() == Ok("dumb");
        Self {
            color_mode: ColorMode::detect(),
            ascii,
            animation: animation && env::var("AXIOMCLI_REDUCED_MOTION").as_deref() != Ok("1"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    User,
    Assistant,
    Reasoning,
    Tool,
    System,
    Error,
}

#[derive(Clone, Debug)]
struct Entry {
    id: u64,
    kind: EntryKind,
    text: String,
    collapsed: bool,
}

#[derive(Debug)]
pub struct TuiState {
    cwd: PathBuf,
    model: String,
    profile: PermissionProfile,
    security: &'static str,
    input: String,
    entries: Vec<Entry>,
    running: bool,
    status: String,
    scroll: u16,
    follow_output: bool,
    focus: Focus,
    selected: Option<usize>,
    search_active: bool,
    search_query: String,
    search_matches: Vec<usize>,
    search_match: usize,
    overlay: Option<Overlay>,
    overlay_scroll: u16,
    options: TuiOptions,
    next_entry_id: u64,
    pending_attention: BTreeMap<String, String>,
    answer_input: String,
    answer_prompt: Option<String>,
}

impl TuiState {
    #[must_use]
    pub fn new(cwd: PathBuf, model: String, profile: PermissionProfile) -> Self {
        Self::with_options(cwd, model, profile, TuiOptions::from_environment(true))
    }

    #[must_use]
    pub fn with_options(
        cwd: PathBuf,
        model: String,
        profile: PermissionProfile,
        options: TuiOptions,
    ) -> Self {
        Self {
            cwd,
            model,
            profile,
            security: "UNATTESTED DEV",
            input: String::new(),
            entries: vec![Entry {
                id: 1,
                kind: EntryKind::System,
                text:
                    "AxiomCLI is ready. Local actions are controlled by the selected permission profile."
                        .into(),
                collapsed: false,
            }],
            running: false,
            status: "Ready".into(),
            scroll: 0,
            follow_output: true,
            focus: Focus::Composer,
            selected: None,
            search_active: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match: 0,
            overlay: None,
            overlay_scroll: 0,
            options,
            next_entry_id: 2,
            pending_attention: BTreeMap::new(),
            answer_input: String::new(),
            answer_prompt: None,
        }
    }

    fn push_entry(&mut self, kind: EntryKind, text: String, collapsed: bool) {
        let id = self.next_entry_id;
        self.next_entry_id = self.next_entry_id.saturating_add(1);
        self.entries.push(Entry {
            id,
            kind,
            text,
            collapsed,
        });
        self.refresh_search();
    }

    fn apply(&mut self, event: &AppEvent) {
        match event {
            AppEvent::PromptAccepted { text, .. } => {
                self.push_entry(EntryKind::User, sanitize_terminal_text(text), false);
            }
            AppEvent::TurnStarted { .. } => {
                self.running = true;
                self.status = "Working…".into();
            }
            AppEvent::TextDelta { text, .. } => {
                self.append_stream(EntryKind::Assistant, text);
                self.status = "Responding…".into();
            }
            AppEvent::ReasoningDelta { text, .. } => {
                self.append_stream(EntryKind::Reasoning, text);
                self.status = "Reasoning…".into();
            }
            AppEvent::ToolProposed { name, .. } => {
                self.push_entry(EntryKind::Tool, format!("Proposed {name}"), false);
                self.status = format!("Preparing {name}…");
            }
            AppEvent::ToolStarted { call_id, name } => {
                self.push_entry(
                    EntryKind::Tool,
                    format!("Running {name} · {call_id}"),
                    false,
                );
                self.status = format!("Running {name}…");
            }
            AppEvent::ToolOutput {
                content, truncated, ..
            } => {
                let suffix = if *truncated {
                    "\n… output truncated"
                } else {
                    ""
                };
                self.push_entry(
                    EntryKind::Tool,
                    format!("{}{suffix}", sanitize_terminal_text(content)),
                    content.lines().count() > 8,
                );
            }
            AppEvent::ToolCompleted { success, .. } => {
                self.status = if *success {
                    "Tool completed".into()
                } else {
                    "Tool failed".into()
                };
            }
            AppEvent::PermissionRequired {
                request_id,
                explanation,
            } => {
                self.pending_attention
                    .insert(format!("permission:{request_id}"), explanation.clone());
                self.status = format!("Permission required: {explanation}");
            }
            AppEvent::PermissionResolved { request_id, .. } => {
                self.pending_attention
                    .remove(&format!("permission:{request_id}"));
            }
            AppEvent::ProviderStatusChanged { connected, detail } => {
                self.status = if *connected {
                    sanitize_terminal_text(detail)
                } else {
                    format!("Provider unavailable: {}", sanitize_terminal_text(detail))
                };
            }
            AppEvent::PermissionProfileChanged { profile } => self.profile = *profile,
            AppEvent::ProgressUpdated { message, .. } => {
                self.status = sanitize_terminal_text(message);
            }
            AppEvent::TaskListUpdated { items } => {
                self.push_entry(
                    EntryKind::System,
                    format!(
                        "TASKS\n{}",
                        items
                            .iter()
                            .map(|item| format!(
                                "[{}] {} · {}",
                                match item.status {
                                    crate::app::TaskStatus::Pending => " ",
                                    crate::app::TaskStatus::InProgress => ">",
                                    crate::app::TaskStatus::Completed => "x",
                                },
                                sanitize_terminal_text(&item.title),
                                sanitize_terminal_text(&item.id)
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    items.len() > 8,
                );
            }
            AppEvent::DiffAvailable {
                diff, truncated, ..
            } => {
                self.push_entry(
                    EntryKind::Tool,
                    format!(
                        "DIFF\n{}{}",
                        sanitize_terminal_text(diff),
                        if *truncated {
                            "\n… diff truncated"
                        } else {
                            ""
                        }
                    ),
                    diff.lines().count() > 12,
                );
            }
            AppEvent::QuestionAsked {
                prompt, options, ..
            } => {
                self.push_entry(
                    EntryKind::System,
                    format!(
                        "QUESTION\n{}\n{}",
                        sanitize_terminal_text(prompt),
                        options.join(" · ")
                    ),
                    false,
                );
            }
            AppEvent::QuestionsAsked { request } => {
                self.pending_attention.insert(
                    format!("question:{}", request.request_id),
                    format!("{} question(s)", request.questions.len()),
                );
                self.push_entry(
                    EntryKind::System,
                    format!(
                        "QUESTIONS\n{}",
                        request
                            .questions
                            .iter()
                            .map(|question| sanitize_terminal_text(&question.prompt))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                    false,
                );
            }
            AppEvent::QuestionsAnswered { request_id, .. } => {
                self.pending_attention
                    .remove(&format!("question:{request_id}"));
            }
            AppEvent::PlanProposed {
                plan_id,
                revision,
                markdown,
            } => {
                self.pending_attention.insert(
                    format!("plan:{plan_id}"),
                    format!("plan r{revision} review"),
                );
                self.push_entry(
                    EntryKind::System,
                    format!("PLAN r{revision}\n{}", sanitize_terminal_text(markdown)),
                    markdown.lines().count() > 12,
                );
            }
            AppEvent::BackgroundTaskChanged { task_id, state } => {
                let key = format!("background:{task_id}");
                if state == "running" || state == "stopping" {
                    self.pending_attention.insert(key, state.clone());
                } else {
                    self.pending_attention.remove(&key);
                }
                self.push_entry(
                    EntryKind::Tool,
                    format!("BACKGROUND {task_id}: {}", sanitize_terminal_text(state)),
                    false,
                );
            }
            AppEvent::PlanReviewed { plan_id, .. } => {
                self.pending_attention.remove(&format!("plan:{plan_id}"));
            }
            AppEvent::WarningRaised { message } => {
                self.push_entry(
                    EntryKind::System,
                    format!("WARNING: {}", sanitize_terminal_text(message)),
                    false,
                );
            }
            AppEvent::TurnCompleted { .. } => {
                self.running = false;
                self.status = "Ready".into();
            }
            AppEvent::TurnCancelled { .. } => {
                self.running = false;
                self.status = "Cancelled".into();
            }
            AppEvent::ErrorRaised { message, .. } => {
                self.running = false;
                self.status = "Error".into();
                self.push_entry(EntryKind::Error, sanitize_terminal_text(message), false);
            }
            _ => {}
        }
        if self.follow_output {
            self.scroll = u16::MAX;
        }
    }

    fn append_stream(&mut self, kind: EntryKind, text: &str) {
        let text = sanitize_terminal_text(text);
        if let Some(last) = self.entries.last_mut()
            && last.kind == kind
        {
            last.text.push_str(&text);
        } else {
            self.push_entry(kind, text, false);
        }
    }

    /// Builds the transcript for a known content width so wrapping happens here
    /// rather than in `Paragraph`: continuation lines keep the body indent, and
    /// wide glyphs are measured by display width instead of by `char` count.
    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        let theme = Theme::for_mode(self.options.color_mode);
        let body_width = usize::from(width).saturating_sub(BODY_INDENT).max(4);
        let indent = " ".repeat(BODY_INDENT);
        let mut lines = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            let (marker, color, label) = match entry.kind {
                EntryKind::User => (self.glyph("›", ">"), theme.cherry_bright, "YOU"),
                EntryKind::Assistant => (self.glyph("◆", "*"), theme.cherry, "AXIOM"),
                EntryKind::Reasoning => (self.glyph("◇", "~"), theme.muted, "THINKING"),
                EntryKind::Tool => (self.glyph("▸", "-"), theme.warning, "TOOL"),
                EntryKind::System => (self.glyph("▪", "+"), theme.cherry, "SYSTEM"),
                EntryKind::Error => (self.glyph("▲", "!"), theme.error, "ERROR"),
            };
            let mut label_style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            if self.selected == Some(index) {
                label_style = label_style.fg(theme.text).bg(theme.cherry_deep);
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
            for source in body.lines() {
                for wrapped in wrap_to_width(source, body_width) {
                    lines.push(Line::from(Span::styled(
                        format!("{indent}{wrapped}"),
                        body_style,
                    )));
                }
            }
            if entry.collapsed {
                lines.push(Line::from(Span::styled(
                    format!("{indent}… select and press Enter to expand, or v to view"),
                    Style::default().fg(theme.muted),
                )));
            }
        }
        lines
    }

    fn glyph<'a>(&self, unicode: &'a str, ascii: &'a str) -> &'a str {
        if self.options.ascii { ascii } else { unicode }
    }

    fn refresh_search(&mut self) {
        let needle = self.search_query.to_lowercase();
        self.search_matches = if needle.is_empty() {
            Vec::new()
        } else {
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    entry.text.to_lowercase().contains(&needle).then_some(index)
                })
                .collect()
        };
        self.search_match = self
            .search_match
            .min(self.search_matches.len().saturating_sub(1));
    }

    fn select_search_match(&mut self) {
        if let Some(index) = self.search_matches.get(self.search_match).copied() {
            self.selected = Some(index);
            self.focus = Focus::Transcript;
            self.follow_output = false;
            self.scroll = u16::try_from(index.saturating_mul(3)).unwrap_or(u16::MAX);
        }
    }

    fn next_match(&mut self, backwards: bool) {
        if self.search_matches.is_empty() {
            return;
        }
        if backwards {
            self.search_match = self
                .search_match
                .checked_sub(1)
                .unwrap_or(self.search_matches.len() - 1);
        } else {
            self.search_match = (self.search_match + 1) % self.search_matches.len();
        }
        self.select_search_match();
    }

    fn move_selection(&mut self, down: bool) {
        if self.entries.is_empty() {
            return;
        }
        let current = self
            .selected
            .unwrap_or_else(|| self.entries.len().saturating_sub(1));
        let next = if down {
            (current + 1).min(self.entries.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        self.selected = Some(next);
        self.focus = Focus::Transcript;
        self.follow_output = false;
        self.scroll = u16::try_from(next.saturating_mul(3)).unwrap_or(u16::MAX);
    }

    fn toggle_selected(&mut self) {
        if let Some(entry) = self.selected.and_then(|index| self.entries.get_mut(index)) {
            entry.collapsed = !entry.collapsed;
        }
    }
}

enum UiMessage {
    Event(AppEvent),
    Done(TurnId, Result<()>),
    Approval(ApprovalRequest, oneshot::Sender<ApprovalResponse>),
    Questions(
        QuestionRequest,
        oneshot::Sender<Result<BTreeMap<String, Vec<String>>>>,
    ),
}

#[derive(Clone)]
struct TuiApproval {
    tx: mpsc::Sender<UiMessage>,
}

#[async_trait]
impl ApprovalHandler for TuiApproval {
    async fn request(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
    ) -> Result<ApprovalResponse> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UiMessage::Approval(request, tx))
            .await
            .map_err(|_| AxiomError::Cancelled)?;
        tokio::select! {
            () = cancellation.cancelled() => Err(AxiomError::Cancelled),
            response = rx => Ok(response.unwrap_or_else(|_| ApprovalResponse::deny())),
        }
    }
}

#[derive(Clone)]
struct TuiQuestions {
    tx: mpsc::Sender<UiMessage>,
}

#[async_trait]
impl QuestionHandler for TuiQuestions {
    async fn request(
        &self,
        request: QuestionRequest,
        cancellation: CancellationToken,
    ) -> Result<BTreeMap<String, Vec<String>>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(UiMessage::Questions(request, tx))
            .await
            .map_err(|_| AxiomError::Cancelled)?;
        tokio::select! {
            () = cancellation.cancelled() => Err(AxiomError::Cancelled),
            response = rx => response.unwrap_or(Err(AxiomError::Cancelled)),
        }
    }
}

struct PendingQuestions {
    request: QuestionRequest,
    index: usize,
    answers: BTreeMap<String, Vec<String>>,
    response: oneshot::Sender<Result<BTreeMap<String, Vec<String>>>>,
}

fn question_status(pending: &PendingQuestions) -> String {
    let question = &pending.request.questions[pending.index];
    let options = question
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| format!("{}={option}", index + 1))
        .collect::<Vec<_>>()
        .join(" · ");
    format!(
        "Question {}/{}: {}{}",
        pending.index + 1,
        pending.request.questions.len(),
        question.prompt,
        if options.is_empty() {
            String::new()
        } else {
            format!(" · {options}")
        }
    )
}

fn parse_tui_answer(input: &str, question: &crate::app::QuestionSpec) -> Result<Vec<String>> {
    let values: Vec<_> = input
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .ok()
                .and_then(|index| question.options.get(index.saturating_sub(1)))
                .cloned()
                .unwrap_or_else(|| value.to_owned())
        })
        .collect();
    if values.is_empty() || (!question.multiple && values.len() != 1) {
        return Err(AxiomError::Tool(
            "enter one answer, or comma-separated answers when multiple selections are allowed"
                .into(),
        ));
    }
    if !question.options.is_empty() && values.iter().any(|value| !question.options.contains(value))
    {
        return Err(AxiomError::Tool(
            "answer must be one of the numbered options".into(),
        ));
    }
    Ok(values)
}

fn approval_status(request: &ApprovalRequest) -> String {
    let mut choices = String::from("[y] allow once");
    if request.allow_session_grants {
        choices.push_str("  [e] exact for session");
    }
    if let Some(scope) = &request.suggested_prefix_scope {
        let _ = write!(choices, "  [p] {scope} for session");
    }
    choices.push_str("  [n] deny");
    format!("{}  {choices}", request.explanation)
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

pub async fn run(
    runner: Arc<dyn TurnRunner>,
    cwd: PathBuf,
    model: String,
    profile: PermissionProfile,
    options: TuiOptions,
    store: Option<SessionStore>,
    resume: Option<SessionId>,
) -> Result<()> {
    let cwd = cwd.canonicalize()?;
    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let runtime = Runtime::new(512);
    let mut state = TuiState::with_options(cwd.clone(), model, profile, options);
    let session_id = if let Some(session_id) = resume {
        let store = store
            .as_ref()
            .ok_or_else(|| AxiomError::Storage("session resume requires a durable store".into()))?;
        let summary = store.summary(&session_id)?;
        if summary.cwd.canonicalize()? != cwd {
            return Err(AxiomError::Storage(format!(
                "resume cwd does not match journaled workspace {}",
                summary.cwd.display()
            )));
        }
        let loaded = store.load_recovering(&session_id)?;
        runtime.restore(&loaded.events).await?;
        runner.restore_session(&session_id, &loaded.events).await?;
        for envelope in &loaded.events {
            state.apply(&envelope.event);
        }
        state.running = false;
        state.status = "Resumed · no interrupted actions replayed".into();
        let resumed = runtime
            .emit(
                session_id.clone(),
                AppEvent::SessionResumed {
                    cwd: cwd.clone(),
                    origin: Origin::Tui,
                    profile,
                },
            )
            .await?;
        store.append(&resumed)?;
        state.apply(&resumed.event);
        let recovery = store.recovery_status(&session_id)?;
        let mut notices = loaded.warnings;
        if recovery.interrupted_turns > 0 || !recovery.interrupted_tools.is_empty() {
            notices.push(format!(
                "Recovered {} interrupted turn(s) and {} tool/task operation(s) with unknown status; nothing was replayed.",
                recovery.interrupted_turns,
                recovery.interrupted_tools.len()
            ));
        }
        if !recovery.changed_paths.is_empty() {
            notices.push(format!(
                "Workspace changes recorded in this session: {}",
                recovery
                    .changed_paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if summary.history_truncated {
            notices.push("Older session history was removed by retention policy.".into());
        }
        for message in notices {
            let warning = runtime
                .emit(session_id.clone(), AppEvent::WarningRaised { message })
                .await?;
            store.append(&warning)?;
            state.apply(&warning.event);
        }
        session_id
    } else {
        let session_id = runtime.session_id();
        let created = runtime
            .dispatch(AppCommand::CreateSession {
                session_id: session_id.clone(),
                cwd: cwd.clone(),
                origin: Origin::Tui,
                profile,
            })
            .await?;
        if let Some(store) = &store {
            store.append_all(&created)?;
        }
        for event in created {
            state.apply(&event.event);
        }
        session_id
    };

    let mut input_events = EventStream::new();
    let (ui_tx, mut ui_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let mut cancellation: Option<CancellationToken> = None;
    let mut pending_approval: Option<(ApprovalRequest, oneshot::Sender<ApprovalResponse>)> = None;
    let mut pending_questions: Option<PendingQuestions> = None;

    assert!(
        !(cfg!(debug_assertions) && env::var("AXIOMCLI_TEST_TUI_PANIC").as_deref() == Ok("1")),
        "requested terminal restoration probe"
    );

    loop {
        terminal.draw(|frame| render(frame, &state))?;
        tokio::select! {
            maybe_event = input_events.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                match event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if pending_questions.is_some() {
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let pending = pending_questions.take().expect("pending questions exist");
                                    let _ = pending.response.send(Err(AxiomError::Cancelled));
                                    state.answer_input.clear();
                                    state.answer_prompt = None;
                                    if let Some(token) = &cancellation {
                                        token.cancel();
                                    }
                                }
                                KeyCode::Esc => {
                                    let pending = pending_questions.take().expect("pending questions exist");
                                    let _ = pending.response.send(Err(AxiomError::Cancelled));
                                    state.answer_input.clear();
                                    state.answer_prompt = None;
                                }
                                KeyCode::Enter => {
                                    let pending = pending_questions.as_mut().expect("pending questions exist");
                                    let question = &pending.request.questions[pending.index];
                                    match parse_tui_answer(&state.answer_input, question) {
                                        Ok(values) => {
                                            pending.answers.insert(question.id.clone(), values);
                                            pending.index += 1;
                                            state.answer_input.clear();
                                            if pending.index == pending.request.questions.len() {
                                                let completed = pending_questions.take().expect("pending questions exist");
                                                let _ = completed.response.send(Ok(completed.answers));
                                                state.answer_prompt = None;
                                                state.status = "Answers submitted".into();
                                            } else {
                                                state.answer_prompt = Some(question_status(pending));
                                                state.status = state.answer_prompt.clone().unwrap_or_default();
                                            }
                                        }
                                        Err(error) => state.status = error.to_string(),
                                    }
                                }
                                KeyCode::Backspace => { state.answer_input.pop(); }
                                KeyCode::Char(character) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
                                    state.answer_input.push(character);
                                }
                                _ => {}
                            }
                        } else if let Some((request, _)) = &pending_approval {
                            match key.code {
                                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                    let (_, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse::deny());
                                    if let Some(token) = &cancellation {
                                        token.cancel();
                                    }
                                    state.status = "Cancelling…".into();
                                }
                                KeyCode::Char('y' | 'Y') => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse { choice: ApprovalChoice::AllowOnce });
                                    state.status = format!("Allowed once · {}", request.request_id);
                                }
                                KeyCode::Char('e' | 'E') if request.allow_session_grants => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse { choice: ApprovalChoice::AllowExactSession });
                                    state.status = format!("Allowed exact operation for session · {}", request.request_id);
                                }
                                KeyCode::Char('p' | 'P') if request.suggested_prefix_scope.is_some() => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse { choice: ApprovalChoice::AllowPrefixSession });
                                    state.status = format!("Allowed safe scope for session · {}", request.request_id);
                                }
                                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse::deny());
                                    state.status = format!("Denied {}", request.request_id);
                                }
                                _ => {
                                    state.status = approval_status(request);
                                }
                            }
                        } else if state.overlay.is_some() {
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('v' | 'q') | KeyCode::Enter => {
                                    state.overlay = None;
                                    state.overlay_scroll = 0;
                                }
                                KeyCode::Up => state.overlay_scroll = state.overlay_scroll.saturating_sub(1),
                                KeyCode::Down => state.overlay_scroll = state.overlay_scroll.saturating_add(1),
                                _ => {}
                            }
                        } else if state.search_active {
                            match key.code {
                                KeyCode::Esc => {
                                    state.search_active = false;
                                    state.search_query.clear();
                                    state.refresh_search();
                                    state.status = "Search cancelled".into();
                                }
                                KeyCode::Enter => {
                                    state.search_active = false;
                                    state.select_search_match();
                                    state.status = format!("{} transcript match(es)", state.search_matches.len());
                                }
                                KeyCode::Backspace => {
                                    state.search_query.pop();
                                    state.refresh_search();
                                }
                                KeyCode::Char(character) if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT => {
                                    state.search_query.push(character);
                                    state.refresh_search();
                                }
                                _ => {}
                            }
                        } else if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                            if let Some(token) = &cancellation {
                                token.cancel();
                                state.status = "Cancelling…".into();
                            } else {
                                break;
                            }
                        } else {
                            match key.code {
                                KeyCode::Esc if !state.running => break,
                                KeyCode::Char('/') if state.focus == Focus::Transcript || state.input.is_empty() => {
                                    state.search_active = true;
                                    state.search_query.clear();
                                    state.refresh_search();
                                    state.status = "Search transcript".into();
                                }
                                KeyCode::Char('?') if state.input.is_empty() => {
                                    state.overlay = Some(Overlay::Help);
                                }
                                KeyCode::Char('n') if state.focus == Focus::Transcript && !state.search_matches.is_empty() => state.next_match(false),
                                KeyCode::Char('N') if state.focus == Focus::Transcript && !state.search_matches.is_empty() => state.next_match(true),
                                KeyCode::Char('j') if state.focus == Focus::Transcript => state.move_selection(true),
                                KeyCode::Char('k') if state.focus == Focus::Transcript => state.move_selection(false),
                                KeyCode::Char('v') if state.focus == Focus::Transcript && state.selected.is_some() => {
                                    state.overlay = state.selected.map(Overlay::Entry);
                                    state.overlay_scroll = 0;
                                }
                                KeyCode::Tab => {
                                    state.focus = if state.focus == Focus::Composer { Focus::Transcript } else { Focus::Composer };
                                    if state.focus == Focus::Transcript && state.selected.is_none() {
                                        state.selected = state.entries.len().checked_sub(1);
                                    }
                                }
                                KeyCode::Enter if state.focus == Focus::Transcript => state.toggle_selected(),
                                KeyCode::Enter if !state.running && !state.input.trim().is_empty() => {
                                    let prompt = std::mem::take(&mut state.input);
                                    let turn_id = runtime.turn_id();
                                    let submitted = runtime.dispatch(AppCommand::SubmitPrompt {
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        text: prompt.clone(),
                                    }).await?;
                                    if let Some(store) = &store {
                                        store.append_all(&submitted)?;
                                    }
                                    for event in submitted {
                                        state.apply(&event.event);
                                    }
                                    let token = CancellationToken::new();
                                    cancellation = Some(token.clone());
                                    let task_runner = runner.clone();
                                    let task_tx = ui_tx.clone();
                                    let context = TurnContext {
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        cwd: cwd.clone(),
                                        permission_profile: profile,
                                        approval: Some(Arc::new(TuiApproval { tx: task_tx.clone() })),
                                        questions: Some(Arc::new(TuiQuestions { tx: task_tx.clone() })),
                                    };
                                    tokio::spawn(async move {
                                        let (event_tx, mut event_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                                        let forwarding = task_tx.clone();
                                        let forward = tokio::spawn(async move {
                                            while let Some(event) = event_rx.recv().await {
                                                let _ = forwarding.send(UiMessage::Event(event)).await;
                                            }
                                        });
                                        let result = task_runner.run(context, prompt, event_tx, token).await;
                                        let _ = forward.await;
                                        let _ = task_tx.send(UiMessage::Done(turn_id, result)).await;
                                    });
                                }
                                KeyCode::Char(character) if state.focus == Focus::Composer && (!state.running || key.modifiers.is_empty()) => state.input.push(character),
                                KeyCode::Backspace if state.focus == Focus::Composer => { state.input.pop(); }
                                KeyCode::Up => {
                                    state.follow_output = false;
                                    state.scroll = state.scroll.saturating_sub(3);
                                    if state.focus == Focus::Transcript { state.move_selection(false); }
                                }
                                KeyCode::Down => {
                                    state.scroll = state.scroll.saturating_add(3);
                                    if state.scroll == u16::MAX { state.follow_output = true; }
                                    if state.focus == Focus::Transcript { state.move_selection(true); }
                                }
                                KeyCode::End => { state.follow_output = true; state.scroll = u16::MAX; }
                                _ => {}
                            }
                        }
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            state.follow_output = false;
                            state.scroll = state.scroll.saturating_sub(3);
                        }
                        MouseEventKind::ScrollDown => state.scroll = state.scroll.saturating_add(3),
                        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                            let (width, height) = crossterm::terminal::size()?;
                            match focus_at(
                                Rect::new(0, 0, width, height),
                                mouse.column,
                                mouse.row,
                            ) {
                                Some(Focus::Composer) => state.focus = Focus::Composer,
                                Some(Focus::Transcript) => {
                                    state.focus = Focus::Transcript;
                                    if state.selected.is_none() {
                                        state.selected = state.entries.len().checked_sub(1);
                                    }
                                }
                                None => {}
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
            message = ui_rx.recv() => {
                let Some(message) = message else { break };
                match message {
                    UiMessage::Event(event) => {
                        let envelope = runtime.emit(session_id.clone(), event.clone()).await?;
                        if let Some(store) = &store {
                            store.append(&envelope)?;
                        }
                        state.apply(&event);
                    }
                    UiMessage::Done(turn_id, result) => {
                        if result.is_err()
                            && let Some(token) = &cancellation
                        {
                            token.cancel();
                        }
                        pending_approval = None;
                        if let Some(pending) = pending_questions.take() {
                            let _ = pending.response.send(Err(AxiomError::Cancelled));
                        }
                        state.answer_input.clear();
                        state.answer_prompt = None;
                        cancellation = None;
                        let command = match result {
                            Ok(()) => AppCommand::FinishTurn { session_id: session_id.clone(), turn_id },
                            Err(AxiomError::Cancelled) => AppCommand::CancelTurn { session_id: session_id.clone(), turn_id },
                            Err(error) => AppCommand::FailTurn { session_id: session_id.clone(), turn_id, message: error.to_string() },
                        };
                        let completed = runtime.dispatch(command).await?;
                        if let Some(store) = &store {
                            store.append_all(&completed)?;
                        }
                        for event in completed {
                            state.apply(&event.event);
                        }
                    }
                    UiMessage::Approval(request, response) => {
                        state.status = approval_status(&request);
                        pending_approval = Some((request, response));
                    }
                    UiMessage::Questions(request, response) => {
                        if pending_questions.is_some() {
                            let _ = response.send(Err(AxiomError::InvalidTransition(
                                "another question request is already pending".into(),
                            )));
                        } else {
                            let pending = PendingQuestions {
                                request,
                                index: 0,
                                answers: BTreeMap::new(),
                                response,
                            };
                            state.answer_input.clear();
                            state.answer_prompt = Some(question_status(&pending));
                            state.status = state.answer_prompt.clone().unwrap_or_default();
                            pending_questions = Some(pending);
                        }
                    }
                }
            }
        }
    }

    let closed = runtime
        .dispatch(AppCommand::CloseSession {
            session_id: session_id.clone(),
        })
        .await?;
    if let Some(store) = &store {
        store.append_all(&closed)?;
    }

    Ok(())
}

pub fn render(frame: &mut Frame<'_>, state: &TuiState) {
    let area = frame.area();
    let theme = Theme::for_mode(state.options.color_mode);
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
            .style(Style::default().fg(theme.cherry_bright))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let rows = main_rows(area);
    render_header(frame, state, theme, rows[0]);
    render_transcript(frame, state, rows[1]);
    render_status(frame, state, theme, rows[2]);
    render_composer(frame, state, theme, rows[3]);
    render_hints(frame, state, theme, rows[4]);

    if let Some(overlay) = &state.overlay {
        render_overlay(frame, state, overlay, theme);
    }
}

fn main_rows(area: Rect) -> [Rect; 5] {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);
    [rows[0], rows[1], rows[2], rows[3], rows[4]]
}

/// Trims `inset` columns from both sides, or `None` when nothing would be left.
fn inset_horizontal(area: Rect, inset: u16) -> Option<Rect> {
    let width = area.width.checked_sub(inset.saturating_mul(2))?;
    (width > 0).then(|| Rect {
        x: area.x.saturating_add(inset),
        y: area.y,
        width,
        height: area.height,
    })
}

fn focus_at(area: Rect, column: u16, row: u16) -> Option<Focus> {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        return None;
    }
    let rows = main_rows(area);
    let position = Position::new(column, row);
    if rows[3].contains(position) {
        Some(Focus::Composer)
    } else if rows[1].contains(position) {
        Some(Focus::Transcript)
    } else {
        None
    }
}

fn render_header(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border))
        .padding(Padding::horizontal(GUTTER));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let width = usize::from(inner.width);

    let chip = format!(" {} ", state.security);
    let chip_width = display_width(&chip);
    let model = state.model.clone();
    let profile = state.profile.to_string();
    let tail_width = 2 + display_width(&model) + 3 + display_width(&profile);
    // The security chip must never be the thing that gets clipped, so the model
    // and profile are the first to go when the header runs out of room.
    let compact = chip_width + tail_width + 12 > width;

    let mut right = vec![Span::styled(
        chip,
        Style::default()
            .fg(theme.on_accent)
            .bg(theme.cherry)
            .add_modifier(Modifier::BOLD),
    )];
    let mut right_width = chip_width;
    if !compact {
        right.push(Span::raw("  "));
        right.push(Span::styled(model, Style::default().fg(theme.text)));
        right.push(Span::styled(" · ", Style::default().fg(theme.muted)));
        right.push(Span::styled(profile, Style::default().fg(theme.muted)));
        right_width += tail_width;
    }

    let brand = "AXIOMCLI";
    let mut left = vec![Span::styled(
        brand,
        Style::default()
            .fg(theme.cherry)
            .add_modifier(Modifier::BOLD),
    )];
    let mut left_width = display_width(brand);
    let cwd_budget = width.saturating_sub(right_width + left_width + 4);
    if cwd_budget >= 16 {
        let cwd = truncate_middle(&state.cwd.display().to_string(), cwd_budget);
        left_width += 2 + display_width(&cwd);
        left.push(Span::raw("  "));
        left.push(Span::styled(cwd, Style::default().fg(theme.muted)));
    }

    let gap = width.saturating_sub(left_width + right_width);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn render_transcript(frame: &mut Frame<'_>, state: &TuiState, area: Rect) {
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
    let scroll = if state.follow_output || state.scroll == u16::MAX {
        max_scroll
    } else {
        state.scroll.min(max_scroll)
    };
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

fn render_status(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(inner) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let status_color = if state.running {
        theme.cherry_bright
    } else {
        theme.success
    };
    let status_text = if state.search_active {
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
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if state.options.ascii || !state.options.animation {
                    if state.running { "*" } else { "o" }
                } else if state.running {
                    "●"
                } else {
                    "○"
                },
                Style::default().fg(status_color),
            ),
            Span::styled(format!(" {status_text}"), Style::default().fg(theme.muted)),
        ])),
        inner,
    );
}

fn render_composer(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(area) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let focused = state.focus == Focus::Composer;
    // A filled slab plus a single accent column reads as one control; a full box
    // border would put a second competing line next to the header divider.
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.surface)),
        area,
    );
    frame.render_widget(
        Block::default().style(Style::default().bg(if focused {
            theme.cherry
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

    let Some(inner) = composer_input_row(area) else {
        return;
    };
    let answering = state.answer_prompt.is_some();
    let input = if answering {
        state.answer_input.as_str()
    } else {
        state.input.as_str()
    };
    let field_width = usize::from(inner.width).saturating_sub(2);
    let visible = tail_to_width(input, field_width);
    let body = if input.is_empty() && !focused {
        Span::styled(
            if answering {
                "Tab to answer"
            } else {
                "Tab or click to type"
            },
            Style::default().fg(theme.muted),
        )
    } else {
        Span::styled(visible.clone(), Style::default().fg(theme.text))
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(
                    "{} ",
                    if answering {
                        "?"
                    } else {
                        state.glyph("›", ">")
                    }
                ),
                Style::default().fg(if focused {
                    theme.cherry_bright
                } else {
                    theme.muted
                }),
            ),
            body,
        ]))
        .style(Style::default().fg(theme.text).bg(theme.surface)),
        inner,
    );

    if focused && state.overlay.is_none() && !state.search_active {
        let offset = u16::try_from(display_width(&visible)).unwrap_or(u16::MAX);
        frame.set_cursor_position((
            inner
                .x
                .saturating_add(2)
                .saturating_add(offset)
                .min(inner.x.saturating_add(inner.width.saturating_sub(1))),
            inner.y,
        ));
    }
}

/// The single editable row inside the composer slab: one column for the accent
/// bar, one of padding, and one blank row above and below the text.
fn composer_input_row(area: Rect) -> Option<Rect> {
    let width = area.width.checked_sub(3)?;
    (width > 0 && area.height >= 3).then(|| Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width,
        height: 1,
    })
}

fn render_hints(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(inner) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let separator = if state.options.ascii { " | " } else { " · " };
    let mut spans = Vec::new();
    for (index, (key, label)) in [
        ("Enter", "send/toggle"),
        ("Tab", "focus"),
        ("j/k", "scroll"),
        ("/", "search"),
        ("v", "view"),
        ("?", "help"),
    ]
    .into_iter()
    .enumerate()
    {
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
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn render_overlay(frame: &mut Frame<'_>, state: &TuiState, overlay: &Overlay, theme: Theme) {
    let (title, body): (&str, String) = match overlay {
        Overlay::Help => (
            " Help ",
            "Tab: switch composer/transcript focus\n\
             j/k or arrows: move through transcript\n\
             Enter: send or collapse/expand selected entry\n\
             /: search transcript; n/N: next/previous match\n\
             v: open selected entry in this terminal-viewport viewer\n\
             End: follow streaming output\n\
             Ctrl+C: cancel active work; Esc: close/quit\n\
             Mouse wheel: scroll; click: focus transcript"
                .into(),
        ),
        Overlay::Entry(index) => {
            let body = state.entries.get(*index).map_or_else(
                || "Entry is no longer available".into(),
                |entry| format!("entry {} · id {}\n\n{}", index + 1, entry.id, entry.text),
            );
            (" Viewer · Esc to close ", body)
        }
    };
    // Help is a fixed list, so give it only the rows it needs; the viewer holds
    // arbitrary entry text and keeps the tall panel.
    let area = match overlay {
        Overlay::Help => {
            let rows = u16::try_from(body.lines().count()).unwrap_or(u16::MAX);
            fitted_rect(88, rows.saturating_add(2), frame.area())
        }
        Overlay::Entry(_) => centered_rect(88, 82, frame.area()),
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(Span::styled(
                        title,
                        Style::default()
                            .fg(theme.cherry)
                            .add_modifier(Modifier::BOLD),
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

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn char_width(character: char) -> usize {
    UnicodeWidthChar::width(character).unwrap_or(0)
}

/// Word-wraps to a display width, hard-splitting words that cannot fit on a
/// line of their own. Measuring in display columns keeps wide glyphs such as
/// emoji from pushing text one cell past the right edge.
fn wrap_to_width(line: &str, width: usize) -> Vec<String> {
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
fn split_to_width(word: &str, width: usize) -> Vec<String> {
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
fn tail_to_width(text: &str, width: usize) -> String {
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

/// Centres a panel of an exact row count, clamped to the available height.
fn fitted_rect(width_percent: u16, rows: u16, area: Rect) -> Rect {
    let height = rows.min(area.height.saturating_sub(2)).max(3);
    let outer = centered_rect(width_percent, 100, area);
    Rect {
        x: outer.x,
        y: outer.y + (outer.height.saturating_sub(height)) / 2,
        width: outer.width,
        height,
    }
}

fn centered_rect(width_percent: u16, height_percent: u16, area: Rect) -> Rect {
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

#[must_use]
pub fn sanitize_terminal_text(input: &str) -> String {
    input
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

fn truncate_middle(input: &str, max_chars: usize) -> String {
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

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    #[test]
    fn strips_terminal_control_sequences() {
        assert_eq!(sanitize_terminal_text("ok\u{1b}[31mred\u{7}"), "ok[31mred");
    }

    #[test]
    fn explicit_color_mode_overrides_ambient_no_color() {
        assert_eq!(
            ColorMode::detect_from(Some("truecolor"), true, Some("xterm-256color"), None),
            ColorMode::TrueColor
        );
        assert_eq!(
            ColorMode::detect_from(None, true, Some("xterm-256color"), Some("truecolor")),
            ColorMode::NoColor
        );
    }

    #[test]
    fn mouse_hit_testing_distinguishes_composer_from_transcript() {
        let area = Rect::new(0, 0, 100, 30);
        assert_eq!(focus_at(area, 5, 27), Some(Focus::Composer));
        assert_eq!(focus_at(area, 5, 10), Some(Focus::Transcript));
        assert_eq!(focus_at(area, 5, 29), None);
    }

    #[test]
    fn pending_attention_is_ordered_and_does_not_replace_message_draft() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Workspace,
        );
        state.input = "unfinished draft".into();
        state.apply(&AppEvent::BackgroundTaskChanged {
            task_id: "z-task".into(),
            state: "running".into(),
        });
        state.apply(&AppEvent::QuestionsAsked {
            request: QuestionRequest {
                request_id: "a-question".into(),
                questions: vec![crate::app::QuestionSpec {
                    id: "one".into(),
                    prompt: "Choose".into(),
                    options: vec!["yes".into(), "no".into()],
                    multiple: false,
                }],
            },
        });
        assert_eq!(state.input, "unfinished draft");
        let keys = state.pending_attention.keys().cloned().collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "background:z-task".to_owned(),
                "question:a-question".to_owned()
            ]
        );
        state.apply(&AppEvent::QuestionsAnswered {
            request_id: "a-question".into(),
            answers: BTreeMap::from([("one".into(), vec!["yes".into()])]),
        });
        assert_eq!(state.pending_attention.len(), 1);
    }

    #[test]
    fn renders_at_minimum_and_typical_sizes() {
        for mode in [
            ColorMode::TrueColor,
            ColorMode::Ansi256,
            ColorMode::Ansi16,
            ColorMode::NoColor,
        ] {
            for (width, height) in [(42, 10), (60, 14), (100, 30), (160, 50)] {
                let backend = TestBackend::new(width, height);
                let mut terminal = Terminal::new(backend).expect("terminal");
                let state = TuiState::with_options(
                    PathBuf::from("/tmp/axiom-project/with/a/very/long/path"),
                    "test-model".into(),
                    PermissionProfile::Confirm,
                    TuiOptions {
                        color_mode: mode,
                        ascii: false,
                        animation: false,
                    },
                );
                terminal.draw(|frame| render(frame, &state)).expect("draw");
                let contents = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>();
                assert!(contents.contains("AXIOMCLI"));
                assert!(contents.contains("UNATTESTED DEV"));
                if mode == ColorMode::NoColor {
                    assert!(
                        terminal
                            .backend()
                            .buffer()
                            .content()
                            .iter()
                            .all(|cell| { cell.fg == Color::Reset && cell.bg == Color::Reset })
                    );
                }
            }
        }
    }

    #[test]
    fn undersized_terminal_has_a_plain_actionable_fallback() {
        let state = TuiState::with_options(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Observe,
            TuiOptions {
                color_mode: ColorMode::NoColor,
                ascii: true,
                animation: false,
            },
        );
        let output = render_symbols(24, 5, &state);
        assert!(output.contains("AxiomCLI needs at least"));
        assert!(output.contains("42x10"));
    }

    #[test]
    fn an_error_returns_to_a_composable_recoverable_state() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.running = true;
        state.input = "retry with narrower scope".into();
        state.apply(&AppEvent::ErrorRaised {
            turn_id: Some(TurnId::new()),
            message: "provider disconnected; retry is safe".into(),
        });
        assert!(!state.running);
        assert_eq!(state.focus, Focus::Composer);
        assert_eq!(state.input, "retry with narrower scope");
        assert!(
            state
                .entries
                .last()
                .is_some_and(|entry| entry.kind == EntryKind::Error)
        );
    }

    #[test]
    fn ascii_mode_and_every_semantic_category_render_without_controls() {
        let mut state = TuiState::with_options(
            PathBuf::from("/tmp/🌸/project"),
            "model".into(),
            PermissionProfile::Workspace,
            TuiOptions {
                color_mode: ColorMode::Ansi16,
                ascii: true,
                animation: false,
            },
        );
        let turn = TurnId::new();
        for event in [
            AppEvent::SessionCreated {
                cwd: PathBuf::from("/tmp/project"),
                origin: Origin::Test,
                profile: PermissionProfile::Workspace,
            },
            AppEvent::SessionResumed {
                cwd: PathBuf::from("/tmp/project"),
                origin: Origin::Test,
                profile: PermissionProfile::Workspace,
            },
            AppEvent::PromptAccepted {
                turn_id: turn.clone(),
                text: "hello".into(),
            },
            AppEvent::TurnStarted {
                turn_id: turn.clone(),
            },
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "answer".into(),
            },
            AppEvent::ReasoningDelta {
                turn_id: turn.clone(),
                text: "reason".into(),
            },
            AppEvent::ProgressUpdated {
                message: "progress".into(),
                completed: Some(1),
                total: Some(2),
            },
            AppEvent::TaskListUpdated {
                items: vec![crate::app::TaskItem {
                    id: "inspect".into(),
                    title: "Inspect blossom files".into(),
                    status: crate::app::TaskStatus::InProgress,
                }],
            },
            AppEvent::ToolProposed {
                turn_id: turn.clone(),
                call_id: "c1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({}),
            },
            AppEvent::ToolStarted {
                call_id: "c1".into(),
                name: "read_file".into(),
            },
            AppEvent::ToolOutput {
                call_id: "c1".into(),
                content: "unsafe\u{1b}[31m".into(),
                truncated: false,
            },
            AppEvent::PermissionRequired {
                request_id: "permission".into(),
                explanation: "write src/main.rs".into(),
            },
            AppEvent::PermissionResolved {
                request_id: "permission".into(),
                allowed: false,
                choice: Some("deny".into()),
            },
            AppEvent::ToolCompleted {
                call_id: "c1".into(),
                success: true,
            },
            AppEvent::WorkspaceChanged {
                paths: vec![PathBuf::from("src/main.rs")],
            },
            AppEvent::DiffAvailable {
                call_id: "c2".into(),
                diff: "-old\n+new".into(),
                truncated: false,
            },
            AppEvent::QuestionAsked {
                question_id: "q".into(),
                prompt: "Choose".into(),
                options: vec!["one".into(), "two".into()],
            },
            AppEvent::QuestionAnswered {
                question_id: "q".into(),
                answers: vec!["one".into()],
            },
            AppEvent::PlanProposed {
                plan_id: "p".into(),
                revision: 1,
                markdown: "1. inspect".into(),
            },
            AppEvent::PlanReviewed {
                plan_id: "p".into(),
                revision: 1,
                decision: "approved".into(),
            },
            AppEvent::BackgroundTaskChanged {
                task_id: "b".into(),
                state: "running".into(),
            },
            AppEvent::UsageUpdated {
                input_tokens: 10,
                output_tokens: 2,
            },
            AppEvent::PermissionProfileChanged {
                profile: PermissionProfile::Workspace,
            },
            AppEvent::ProviderStatusChanged {
                connected: true,
                detail: "connected".into(),
            },
            AppEvent::SecurityStatusChanged {
                state: crate::app::SecurityStatus::UnattestedDevelopment,
            },
            AppEvent::WarningRaised {
                message: "careful".into(),
            },
            AppEvent::TurnCompleted {
                turn_id: turn.clone(),
            },
            AppEvent::TurnCancelled {
                turn_id: turn.clone(),
            },
            AppEvent::ErrorRaised {
                turn_id: Some(turn),
                message: "failed".into(),
            },
            AppEvent::SessionClosed,
        ] {
            state.apply(&event);
        }
        let output = render_symbols(100, 30, &state);
        assert!(output.contains("AXIOM"));
        assert!(
            state
                .entries
                .iter()
                .any(|entry| entry.text.contains("DIFF"))
        );
        assert!(output.contains("QUESTION"));
        assert!(!output.contains('\u{1b}'));
        assert!(!output.contains('✿'));
    }

    #[test]
    fn search_selection_folding_and_viewer_are_operational() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.push_entry(EntryKind::Tool, "first output".into(), false);
        state.push_entry(EntryKind::Tool, "needle output".into(), false);
        state.search_query = "needle".into();
        state.refresh_search();
        state.select_search_match();
        assert_eq!(state.selected, Some(2));
        state.toggle_selected();
        assert!(state.entries[2].collapsed);
        state.overlay = state.selected.map(Overlay::Entry);
        state.overlay_scroll = 0;
        assert!(render_symbols(80, 24, &state).contains("needle output"));
    }

    #[test]
    fn streamed_content_does_not_steal_manual_scroll_or_selection() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.follow_output = false;
        state.scroll = 4;
        state.selected = Some(0);
        state.apply(&AppEvent::TextDelta {
            turn_id: TurnId::new(),
            text: "stream".into(),
        });
        assert_eq!(state.scroll, 4);
        assert_eq!(state.selected, Some(0));
    }

    #[test]
    fn typical_operational_document_snapshot() {
        let mut state = TuiState::with_options(
            PathBuf::from("/work/axiomcli"),
            "glm-test".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        );
        let turn = "00000000-0000-0000-0000-000000000001"
            .parse::<TurnId>()
            .expect("turn");
        state.apply(&AppEvent::PromptAccepted {
            turn_id: turn.clone(),
            text: "Inspect the repository".into(),
        });
        state.apply(&AppEvent::TextDelta {
            turn_id: turn,
            text: "I’ll inspect the relevant files.".into(),
        });
        state.apply(&AppEvent::ToolOutput {
            call_id: "tool-1".into(),
            content: "src/main.rs\nsrc/lib.rs".into(),
            truncated: false,
        });
        insta::assert_snapshot!(
            "tui_typical_operational_document",
            render_symbols(80, 20, &state)
        );
    }

    /// Dumps rendered frames (symbols plus resolved colours) to
    /// `target/tui-preview/*.json` so the shell can be reviewed as an image
    /// rather than as a plain-text buffer. Ignored by default:
    /// `cargo test -p axiomcli --lib tui_preview_frames -- --ignored`.
    #[test]
    #[ignore = "writes preview frames for visual review rather than asserting"]
    fn tui_preview_frames() {
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/tui-preview");
        std::fs::create_dir_all(&directory).expect("preview directory");
        for (name, width, height, state) in preview_scenarios() {
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal.draw(|frame| render(frame, &state)).expect("draw");
            let buffer = terminal.backend().buffer();
            let rows = (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| {
                            let cell = &buffer[(x, y)];
                            serde_json::json!({
                                "s": cell.symbol(),
                                "f": preview_rgb(cell.fg),
                                "b": preview_rgb(cell.bg),
                                "d": cell.modifier.contains(Modifier::BOLD),
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let frame = serde_json::json!({"width": width, "height": height, "rows": rows});
            std::fs::write(
                directory.join(format!("{name}.json")),
                serde_json::to_string(&frame).expect("frame json"),
            )
            .expect("write frame");
        }
    }

    fn preview_rgb(color: Color) -> Option<[u8; 3]> {
        match color {
            Color::Rgb(red, green, blue) => Some([red, green, blue]),
            _ => None,
        }
    }

    fn preview_state() -> TuiState {
        TuiState::with_options(
            PathBuf::from("/home/salt/sync2/work/axiomai"),
            "glm-5-2".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        )
    }

    fn preview_scenarios() -> Vec<(&'static str, u16, u16, TuiState)> {
        let mut conversation = preview_state();
        conversation.push_entry(EntryKind::User, "hello axiom whats up".into(), false);
        conversation.push_entry(
            EntryKind::Reasoning,
            "The user is just greeting me casually. I should respond in a friendly, concise way."
                .into(),
            false,
        );
        conversation.push_entry(
            EntryKind::Assistant,
            "Hey! 👋 Not much, just hanging out in your `axiomai` workspace ready to work. \
             What's on your mind? Want me to poke around the repo, fix something, build \
             something, or just go wherever your idea takes you?"
                .into(),
            false,
        );

        let mut working = preview_state();
        working.push_entry(
            EntryKind::User,
            "run the release verification".into(),
            false,
        );
        working.push_entry(
            EntryKind::Tool,
            "shell · scripts/verify-release\n  checking signatures\n  checking manifests".into(),
            false,
        );
        working.push_entry(
            EntryKind::Assistant,
            "Signatures and manifests both verify. Packaging the tarball next.".into(),
            false,
        );
        working.running = true;
        working.status = "Working…".into();
        working
            .pending_attention
            .insert("approval:1".into(), "shell command".into());

        let mut browsing = preview_state();
        browsing.focus = Focus::Transcript;
        browsing.push_entry(
            EntryKind::User,
            "where is the needle handled?".into(),
            false,
        );
        browsing.push_entry(
            EntryKind::Tool,
            "src/policy.rs:120\nsrc/policy.rs:214\nsrc/session.rs:88".into(),
            true,
        );
        browsing.push_entry(
            EntryKind::Error,
            "provider disconnected; retry is safe".into(),
            false,
        );
        browsing.search_query = "needle".into();
        browsing.refresh_search();
        browsing.selected = Some(1);
        browsing.follow_output = false;

        let mut overlay = preview_state();
        overlay.push_entry(EntryKind::User, "help".into(), false);
        overlay.overlay = Some(Overlay::Help);

        let narrow = preview_state();
        let minimum = preview_state();

        vec![
            ("conversation", 100, 30, conversation),
            ("working", 100, 30, working),
            ("browsing", 100, 30, browsing),
            ("overlay", 100, 30, overlay),
            ("narrow", 54, 16, narrow),
            ("minimum", 42, 10, minimum),
        ]
    }

    fn render_symbols(width: u16, height: u16, state: &TuiState) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, state)).expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                let line = (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>();
                line.trim_end().to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn wrapping_measures_display_width_and_breaks_unbreakable_words() {
        assert_eq!(
            wrap_to_width("the quick brown fox", 9),
            vec!["the quick", "brown fox"]
        );
        // A wave emoji is two columns wide, so only three fit in eight columns.
        assert_eq!(wrap_to_width("👋👋👋👋", 8), vec!["👋👋👋👋"]);
        assert_eq!(wrap_to_width("👋👋👋👋", 7), vec!["👋👋👋", "👋"]);
        assert_eq!(wrap_to_width("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap_to_width("    indented", 20), vec!["    indented"]);
        assert_eq!(wrap_to_width("", 10), vec![""]);
    }

    #[test]
    fn composer_keeps_the_caret_visible_for_overlong_input() {
        assert_eq!(tail_to_width("abcdef", 10), "abcdef");
        assert_eq!(tail_to_width("abcdef", 3), "def");
        assert_eq!(tail_to_width("a👋bc", 3), "bc");
    }

    #[test]
    fn transcript_text_never_crosses_the_right_gutter() {
        for (width, height) in [(42, 10), (60, 14), (100, 30), (160, 50)] {
            let mut state = TuiState::with_options(
                PathBuf::from("/tmp/axiom-project"),
                "test-model".into(),
                PermissionProfile::Confirm,
                TuiOptions {
                    color_mode: ColorMode::TrueColor,
                    ascii: false,
                    animation: false,
                },
            );
            state.push_entry(
                EntryKind::Assistant,
                "Hey! 👋 Not much, just hanging out in your workspace ready to work, with a \
                 deliberately-long-unbreakable-token-that-cannot-fit-on-any-single-line-at-all \
                 to force a hard split."
                    .into(),
                false,
            );
            let backend = TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal.draw(|frame| render(frame, &state)).expect("draw");
            let buffer = terminal.backend().buffer();
            // Rows 0..2 hold the header and its full-bleed divider; the rest of
            // the shell must stay inside the two-column gutter.
            for y in 2..height {
                for x in [0, 1, width - 2, width - 1] {
                    let cell = &buffer[(x, y)];
                    assert!(
                        cell.symbol().trim().is_empty(),
                        "ink at ({x},{y}) in a {width}x{height} frame: {:?}",
                        cell.symbol()
                    );
                }
            }
        }
    }

    #[test]
    fn path_truncation_keeps_both_ends() {
        assert_eq!(truncate_middle("abcdefghij", 7), "abc…hij");
    }

    #[tokio::test]
    async fn dropped_tui_approval_surface_fails_closed() {
        let (tx, mut rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
        let handler = TuiApproval { tx };
        let request = ApprovalRequest {
            request_id: "request".into(),
            explanation: "delete /workspace/file".into(),
            effect: crate::policy::Effect::FileDelete {
                path: PathBuf::from("/workspace/file"),
            },
            allow_session_grants: true,
            suggested_prefix_scope: Some("FileDelete within /workspace".into()),
        };
        let task =
            tokio::spawn(async move { handler.request(request, CancellationToken::new()).await });
        let UiMessage::Approval(_, response) = rx.recv().await.expect("approval message") else {
            panic!("unexpected UI message");
        };
        drop(response);
        assert_eq!(
            task.await.expect("join").expect("closed response"),
            ApprovalResponse::deny()
        );
    }
}
