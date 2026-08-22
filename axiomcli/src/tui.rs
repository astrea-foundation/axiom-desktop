use std::{env, fmt::Write as _, io, path::PathBuf, sync::Arc};

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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    AxiomError, Result,
    agent::{TurnContext, TurnRunner},
    app::{AppCommand, AppEvent, Origin, PermissionProfile, Runtime, SessionId, TurnId},
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::SessionStore,
};

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
        if env::var_os("NO_COLOR").is_some() || env::var("TERM").as_deref() == Ok("dumb") {
            return Self::NoColor;
        }
        match env::var("AXIOMCLI_COLOR").as_deref() {
            Ok("none" | "never") => Self::NoColor,
            Ok("16") => Self::Ansi16,
            Ok("256") => Self::Ansi256,
            Ok("truecolor" | "24bit") => Self::TrueColor,
            _ if env::var("COLORTERM")
                .is_ok_and(|value| value.contains("truecolor") || value.contains("24bit")) =>
            {
                Self::TrueColor
            }
            _ if env::var("TERM").is_ok_and(|value| value.contains("256color")) => Self::Ansi256,
            _ => Self::Ansi16,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Theme {
    cherry: Color,
    cherry_bright: Color,
    cherry_deep: Color,
    surface: Color,
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
                cherry_bright: Color::Rgb(255, 107, 133),
                cherry_deep: Color::Rgb(138, 31, 48),
                surface: Color::Rgb(24, 24, 27),
                text: Color::Rgb(244, 244, 246),
                muted: Color::Rgb(127, 127, 136),
                success: Color::Rgb(126, 226, 168),
                warning: Color::Rgb(235, 181, 92),
                error: Color::Rgb(255, 95, 95),
            },
            ColorMode::Ansi256 => Self {
                cherry: Color::Indexed(204),
                cherry_bright: Color::Indexed(211),
                cherry_deep: Color::Indexed(88),
                surface: Color::Indexed(234),
                text: Color::Indexed(255),
                muted: Color::Indexed(244),
                success: Color::Indexed(114),
                warning: Color::Indexed(179),
                error: Color::Indexed(203),
            },
            ColorMode::Ansi16 => Self {
                cherry: Color::LightMagenta,
                cherry_bright: Color::LightRed,
                cherry_deep: Color::Magenta,
                surface: Color::Black,
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
                surface: Color::Reset,
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
                text: if options.ascii {
                    "AxiomCLI is ready. Local actions are controlled by the selected permission profile.".into()
                } else {
                    "✿ AxiomCLI is ready. Local actions are controlled by the selected permission profile.".into()
                },
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
            AppEvent::PermissionRequired { explanation, .. } => {
                self.status = format!("Permission required: {explanation}");
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
            AppEvent::PlanProposed {
                revision, markdown, ..
            } => {
                self.push_entry(
                    EntryKind::System,
                    format!("PLAN r{revision}\n{}", sanitize_terminal_text(markdown)),
                    markdown.lines().count() > 12,
                );
            }
            AppEvent::BackgroundTaskChanged { task_id, state } => {
                self.push_entry(
                    EntryKind::Tool,
                    format!("BACKGROUND {task_id}: {}", sanitize_terminal_text(state)),
                    false,
                );
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

    fn transcript_lines(&self) -> Vec<Line<'static>> {
        let theme = Theme::for_mode(self.options.color_mode);
        let mut lines = Vec::new();
        for (index, entry) in self.entries.iter().enumerate() {
            let (marker, color, label) = match entry.kind {
                EntryKind::User => (self.glyph("›", ">"), theme.cherry_bright, "YOU"),
                EntryKind::Assistant => (self.glyph("◆", "*"), theme.text, "AXIOM"),
                EntryKind::Reasoning => (self.glyph("◇", "."), theme.muted, "THINKING"),
                EntryKind::Tool => (self.glyph("◆", "#"), theme.warning, "TOOL"),
                EntryKind::System => (self.glyph("✿", "+"), theme.cherry, "SYSTEM"),
                EntryKind::Error => ("!", theme.error, "ERROR"),
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!(
                        "{}{} {label} ",
                        if self.selected == Some(index) {
                            ">"
                        } else {
                            " "
                        },
                        marker
                    ),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "{}{}",
                        if entry.collapsed { "[collapsed]" } else { "" },
                        if self.search_matches.contains(&index) {
                            " [match]"
                        } else {
                            ""
                        }
                    ),
                    Style::default().fg(theme.muted),
                ),
            ]));
            if entry.collapsed {
                let preview = entry.text.lines().take(3).collect::<Vec<_>>().join("\n");
                lines.extend(preview.lines().map(|line| Line::from(format!("  {line}"))));
                lines.push(Line::from(Span::styled(
                    "  … select and press Enter to expand, or v to view",
                    Style::default().fg(theme.muted),
                )));
            } else {
                lines.extend(
                    entry
                        .text
                        .lines()
                        .map(|line| Line::from(format!("  {line}"))),
                );
            }
            lines.push(Line::default());
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
}

#[derive(Clone)]
struct TuiApproval {
    tx: mpsc::UnboundedSender<UiMessage>,
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
            .map_err(|_| AxiomError::Cancelled)?;
        tokio::select! {
            () = cancellation.cancelled() => Err(AxiomError::Cancelled),
            response = rx => Ok(response.unwrap_or_else(|_| ApprovalResponse::deny())),
        }
    }
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
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let mut cancellation: Option<CancellationToken> = None;
    let mut pending_approval: Option<(ApprovalRequest, oneshot::Sender<ApprovalResponse>)> = None;

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
                        if let Some((request, _)) = &pending_approval {
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
                                    };
                                    tokio::spawn(async move {
                                        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
                                        let forwarding = task_tx.clone();
                                        let forward = tokio::spawn(async move {
                                            while let Some(event) = event_rx.recv().await {
                                                let _ = forwarding.send(UiMessage::Event(event));
                                            }
                                        });
                                        let result = task_runner.run(context, prompt, event_tx, token).await;
                                        let _ = forward.await;
                                        let _ = task_tx.send(UiMessage::Done(turn_id, result));
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
                            state.focus = Focus::Transcript;
                            if state.selected.is_none() {
                                state.selected = state.entries.len().checked_sub(1);
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
                        pending_approval = None;
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
    if area.width < 42 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("AxiomCLI needs at least 42×10 terminal cells.")
                .style(Style::default().fg(theme.cherry_bright))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

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
    render_header(frame, state, rows[0]);

    let lines = state.transcript_lines();
    let visible_height = usize::from(rows[1].height.saturating_sub(2));
    let max_scroll = u16::try_from(lines.len().saturating_sub(visible_height)).unwrap_or(u16::MAX);
    let scroll = if state.follow_output || state.scroll == u16::MAX {
        max_scroll
    } else {
        state.scroll.min(max_scroll)
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme.cherry_deep)),
            )
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        rows[1],
    );

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
    } else {
        state.status.clone()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if state.options.ascii || !state.options.animation {
                    if state.running { "* " } else { "o " }
                } else if state.running {
                    "● "
                } else {
                    "○ "
                },
                Style::default().fg(status_color),
            ),
            Span::styled(status_text, Style::default().fg(theme.muted)),
        ])),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(state.input.as_str())
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(Span::styled(
                        " › Message ",
                        Style::default().fg(theme.cherry_bright),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if state.running {
                        theme.cherry_deep
                    } else {
                        theme.cherry
                    })),
            ),
        rows[3],
    );
    frame.render_widget(
        Paragraph::new(if state.options.ascii {
            "Enter send/toggle | Tab focus | j/k scroll | / search | v view | ? help"
        } else {
            "Enter send/toggle · Tab focus · j/k scroll · / search · v view · ? help"
        })
        .style(Style::default().fg(theme.muted)),
        rows[4],
    );

    if let Some(overlay) = &state.overlay {
        render_overlay(frame, state, overlay, theme);
    }
}

fn render_header(frame: &mut Frame<'_>, state: &TuiState, area: Rect) {
    let theme = Theme::for_mode(state.options.color_mode);
    let cwd = state.cwd.display().to_string();
    let right = if area.width < 72 {
        state.security.into()
    } else {
        format!("{} · {} · {}", state.security, state.model, state.profile)
    };
    let available = usize::from(area.width);
    let left_max = available.saturating_sub(right.chars().count() + 3);
    let left = if area.width < 72 {
        "AXIOMCLI".into()
    } else {
        truncate_middle(&format!("AXIOMCLI · {cwd}"), left_max)
    };
    let gap = " ".repeat(available.saturating_sub(left.chars().count() + right.chars().count()));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                left,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::raw(gap),
            Span::styled(right, Style::default().fg(theme.cherry_bright)),
        ])),
        area,
    );
}

fn render_overlay(frame: &mut Frame<'_>, state: &TuiState, overlay: &Overlay, theme: Theme) {
    let area = centered_rect(90, 82, frame.area());
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
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(body)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.cherry_bright)),
            )
            .scroll((state.overlay_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
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
    fn path_truncation_keeps_both_ends() {
        assert_eq!(truncate_middle("abcdefghij", 7), "abc…hij");
    }

    #[tokio::test]
    async fn dropped_tui_approval_surface_fails_closed() {
        let (tx, mut rx) = mpsc::unbounded_channel();
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
