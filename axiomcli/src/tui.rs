use std::{
    cell::RefCell, collections::BTreeMap, env, fmt::Write as _, io, path::PathBuf, sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use crossterm::{
    event::{
        DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers, MouseEventKind,
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

mod markdown;
mod task;

use markdown::{MarkdownStyles, StreamingMarkdownRenderer};
use task::{TaskActivity, TaskPhase, TaskRunView, ToolPhase};

/// Every region of the shell is inset by the same number of columns so text,
/// dividers and the composer all line up on one left edge.
const GUTTER: u16 = 2;
/// Columns reserved for the role marker; entry bodies are indented to match so
/// wrapped continuations stay under the first line instead of falling to zero.
const BODY_INDENT: usize = 2;
/// Smallest terminal the full layout is designed for.
const MIN_WIDTH: u16 = 42;
const MIN_HEIGHT: u16 = 10;
const ANIMATION_TICK: Duration = Duration::from_millis(160);

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
    blossom_pale: Color,
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
                blossom_pale: Color::Rgb(255, 199, 210),
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
                blossom_pale: Color::Indexed(218),
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
                blossom_pale: Color::White,
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
                blossom_pale: Color::Reset,
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

fn markdown_styles(theme: Theme, mode: ColorMode) -> MarkdownStyles {
    MarkdownStyles {
        text: Style::default().fg(theme.text),
        muted: Style::default().fg(theme.muted),
        accent: Style::default().fg(theme.cherry),
        accent_bright: Style::default().fg(theme.cherry_bright),
        code: Style::default().fg(theme.cherry_bright).bg(theme.surface),
        code_surface: Style::default().fg(theme.text).bg(theme.surface),
        border: Style::default().fg(theme.border),
        quote: Style::default().fg(theme.cherry_deep),
        link: Style::default()
            .fg(theme.cherry_bright)
            .add_modifier(Modifier::UNDERLINED),
        warning: Style::default().fg(theme.warning),
        success: Style::default().fg(theme.success),
        truecolor: mode == ColorMode::TrueColor,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    None,
    Composer,
    Transcript,
}

fn is_search_shortcut(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL)
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
    Task,
    Reasoning,
    Tool,
    System,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryFormat {
    Plain,
    Markdown,
}

#[derive(Clone, Debug)]
struct Entry {
    id: u64,
    kind: EntryKind,
    format: EntryFormat,
    text: String,
    collapsed: bool,
    markdown: RefCell<StreamingMarkdownRenderer>,
    task: Option<TaskRunView>,
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
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
    exit_armed: bool,
    options: TuiOptions,
    next_entry_id: u64,
    pending_attention: BTreeMap<String, String>,
    answer_input: String,
    answer_prompt: Option<String>,
    animation_frame: u64,
    transition_ticks: u8,
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
                format: EntryFormat::Plain,
                text:
                    "AxiomCLI is ready. Local actions are controlled by the selected permission profile."
                        .into(),
                collapsed: false,
                markdown: RefCell::default(),
                task: None,
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
            exit_armed: false,
            options,
            next_entry_id: 2,
            pending_attention: BTreeMap::new(),
            answer_input: String::new(),
            answer_prompt: None,
            animation_frame: 0,
            transition_ticks: 0,
        }
    }

    fn push_entry(&mut self, kind: EntryKind, text: String, collapsed: bool) {
        let format = if kind == EntryKind::Assistant {
            EntryFormat::Markdown
        } else {
            EntryFormat::Plain
        };
        self.push_entry_with_format(kind, format, text, collapsed);
    }

    fn push_entry_with_format(
        &mut self,
        kind: EntryKind,
        format: EntryFormat,
        text: String,
        collapsed: bool,
    ) {
        let id = self.next_entry_id;
        self.next_entry_id = self.next_entry_id.saturating_add(1);
        self.entries.push(Entry {
            id,
            kind,
            format,
            text,
            collapsed,
            markdown: RefCell::default(),
            task: None,
        });
        self.refresh_search();
    }

    fn push_task(&mut self, turn_id: TurnId, prompt: String) -> usize {
        let id = self.next_entry_id;
        self.next_entry_id = self.next_entry_id.saturating_add(1);
        self.entries.push(Entry {
            id,
            kind: EntryKind::Task,
            format: EntryFormat::Plain,
            text: String::new(),
            collapsed: true,
            markdown: RefCell::default(),
            task: Some(TaskRunView::new(turn_id, prompt)),
        });
        self.entries.len() - 1
    }

    fn task_index(&self, turn_id: &TurnId) -> Option<usize> {
        self.entries.iter().position(|entry| {
            entry
                .task
                .as_ref()
                .is_some_and(|task| task.turn_id == *turn_id)
        })
    }

    fn ensure_task(&mut self, turn_id: &TurnId) -> usize {
        self.task_index(turn_id)
            .unwrap_or_else(|| self.push_task(turn_id.clone(), "Task".into()))
    }

    fn active_task_mut(&mut self) -> Option<&mut TaskRunView> {
        self.entries
            .iter_mut()
            .rev()
            .filter_map(|entry| entry.task.as_mut())
            .find(|task| !task.phase.is_terminal())
    }

    fn task_for_call_mut(&mut self, call_id: &str) -> Option<&mut TaskRunView> {
        self.entries
            .iter_mut()
            .rev()
            .filter_map(|entry| entry.task.as_mut())
            .find(|task| task.tools.iter().any(|tool| tool.call_id == call_id))
    }

    fn resume_active_task(&mut self) {
        if !self.has_pending_user_attention()
            && let Some(task) = self.active_task_mut()
        {
            task.phase = TaskPhase::Running;
            task.activity = task.running_tool().map_or(TaskActivity::Thinking, |tool| {
                TaskActivity::RunningTool(tool.name.clone())
            });
        }
    }

    fn wait_active_task(&mut self, message: String) {
        if let Some(task) = self.active_task_mut() {
            task.phase = TaskPhase::Waiting;
            task.activity = TaskActivity::Waiting(message);
        }
    }

    fn has_pending_user_attention(&self) -> bool {
        self.pending_attention.keys().any(|key| {
            key.starts_with("permission:")
                || key.starts_with("question:")
                || key.starts_with("plan:")
        })
    }

    fn waiting_for_user(&self) -> bool {
        self.has_pending_user_attention()
            || self
                .entries
                .iter()
                .rev()
                .filter_map(|entry| entry.task.as_ref())
                .find(|task| !task.phase.is_terminal())
                .is_some_and(|task| task.phase == TaskPhase::Waiting)
    }

    fn has_background_activity(&self) -> bool {
        self.pending_attention
            .keys()
            .any(|key| key.starts_with("background:"))
    }

    fn apply(&mut self, event: &AppEvent) {
        match event {
            AppEvent::PromptAccepted { turn_id, text } => {
                self.push_entry(EntryKind::User, sanitize_terminal_text(text), false);
                self.push_task(turn_id.clone(), sanitize_terminal_text(text));
            }
            AppEvent::TurnStarted { turn_id } => {
                self.running = true;
                self.status = "Working…".into();
                let index = self.ensure_task(turn_id);
                if let Some(task) = self.entries[index].task.as_mut() {
                    task.phase = TaskPhase::Running;
                    task.activity = TaskActivity::Thinking;
                }
            }
            AppEvent::TextDelta { turn_id, text } => {
                let text = sanitize_terminal_text(text);
                if let Some(index) = self.task_index(turn_id) {
                    if let Some(task) = self.entries[index].task.as_mut() {
                        task.response.push_str(&text);
                        task.activity = TaskActivity::Responding;
                    }
                } else {
                    self.append_stream(EntryKind::Assistant, &text);
                }
                self.status = "Responding…".into();
            }
            AppEvent::ReasoningDelta { turn_id, text } => {
                let text = sanitize_terminal_text(text);
                if let Some(index) = self.task_index(turn_id) {
                    if let Some(task) = self.entries[index].task.as_mut() {
                        task.reasoning.push_str(&text);
                        if task.phase != TaskPhase::Waiting {
                            task.activity = TaskActivity::Thinking;
                        }
                    }
                } else {
                    self.append_stream(EntryKind::Reasoning, &text);
                }
                self.status = "Reasoning…".into();
            }
            AppEvent::ToolProposed {
                turn_id,
                call_id,
                name,
                arguments,
            } => {
                let index = self.ensure_task(turn_id);
                if let Some(task) = self.entries[index].task.as_mut() {
                    task.ensure_tool(call_id, name, arguments.clone());
                    task.activity = TaskActivity::PreparingTool(name.clone());
                }
                self.status = format!("Preparing {name}…");
            }
            AppEvent::ToolStarted { call_id, name } => {
                if let Some(task) = self.task_for_call_mut(call_id) {
                    task.ensure_tool(call_id, name, serde_json::Value::Null)
                        .phase = ToolPhase::Running;
                    task.activity = TaskActivity::RunningTool(name.clone());
                } else if let Some(task) = self.active_task_mut() {
                    task.ensure_tool(call_id, name, serde_json::Value::Null)
                        .phase = ToolPhase::Running;
                    task.activity = TaskActivity::RunningTool(name.clone());
                } else {
                    self.push_entry(
                        EntryKind::Tool,
                        format!("Running {name} · {call_id}"),
                        false,
                    );
                }
                self.status = format!("Running {name}…");
            }
            AppEvent::ToolOutput {
                call_id,
                content,
                truncated,
            } => {
                let content = sanitize_terminal_text(content);
                if let Some(task) = self.task_for_call_mut(call_id) {
                    if let Some(tool) = task.tool_mut(call_id) {
                        tool.output.push_str(&content);
                        tool.output_truncated |= *truncated;
                    }
                } else if let Some(task) = self.active_task_mut() {
                    let tool = task.ensure_tool(call_id, "tool", serde_json::Value::Null);
                    tool.phase = ToolPhase::Running;
                    tool.output.push_str(&content);
                    tool.output_truncated |= *truncated;
                } else {
                    let suffix = if *truncated {
                        "\n… output truncated"
                    } else {
                        ""
                    };
                    self.push_entry(
                        EntryKind::Tool,
                        format!("{content}{suffix}"),
                        content.lines().count() > 8,
                    );
                }
            }
            AppEvent::ToolCompleted { call_id, success } => {
                if let Some(task) = self.task_for_call_mut(call_id) {
                    if let Some(tool) = task.tool_mut(call_id) {
                        tool.phase = if *success {
                            ToolPhase::Completed
                        } else {
                            ToolPhase::Failed
                        };
                    }
                    if task.phase != TaskPhase::Waiting {
                        task.activity =
                            task.running_tool().map_or(TaskActivity::Thinking, |tool| {
                                TaskActivity::RunningTool(tool.name.clone())
                            });
                    }
                }
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
                self.wait_active_task(format!("Waiting for permission · {explanation}"));
            }
            AppEvent::PermissionResolved {
                request_id,
                allowed,
                ..
            } => {
                self.pending_attention
                    .remove(&format!("permission:{request_id}"));
                self.status = if *allowed {
                    "Permission granted · continuing…".into()
                } else {
                    "Permission denied · adjusting…".into()
                };
                self.resume_active_task();
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
                let message = sanitize_terminal_text(message);
                if let Some(task) = self.active_task_mut()
                    && task.phase != TaskPhase::Waiting
                {
                    task.activity = TaskActivity::Working(message.clone());
                }
                self.status = message;
            }
            AppEvent::TaskListUpdated { items } => {
                let sanitized = items
                    .iter()
                    .cloned()
                    .map(|mut item| {
                        item.title = sanitize_terminal_text(&item.title);
                        item.id = sanitize_terminal_text(&item.id);
                        item
                    })
                    .collect::<Vec<_>>();
                if let Some(task) = self.active_task_mut() {
                    task.tasks = sanitized;
                } else {
                    self.push_entry(
                        EntryKind::System,
                        format!("TASKS\n{} item(s)", sanitized.len()),
                        sanitized.len() > 8,
                    );
                }
            }
            AppEvent::DiffAvailable {
                call_id,
                diff,
                truncated,
            } => {
                let diff = sanitize_terminal_text(diff);
                if let Some(task) = self.task_for_call_mut(call_id) {
                    if let Some(tool) = task.tool_mut(call_id) {
                        tool.diff.push_str(&diff);
                        tool.diff_truncated |= *truncated;
                    }
                } else if let Some(task) = self.active_task_mut() {
                    let tool = task.ensure_tool(call_id, "edit", serde_json::Value::Null);
                    tool.diff.push_str(&diff);
                    tool.diff_truncated |= *truncated;
                } else {
                    self.push_entry(
                        EntryKind::Tool,
                        format!(
                            "DIFF\n{diff}{}",
                            if *truncated {
                                "\n… diff truncated"
                            } else {
                                ""
                            }
                        ),
                        diff.lines().count() > 12,
                    );
                }
            }
            AppEvent::WorkspaceChanged { paths } => {
                if let Some(task) = self.active_task_mut() {
                    for path in paths {
                        if !task.changed_paths.contains(path) {
                            task.changed_paths.push(path.clone());
                        }
                    }
                }
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
                self.wait_active_task("Waiting for your answer".into());
            }
            AppEvent::QuestionAnswered { .. } => self.resume_active_task(),
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
                self.wait_active_task("Waiting for your answer".into());
            }
            AppEvent::QuestionsAnswered { request_id, .. } => {
                self.pending_attention
                    .remove(&format!("question:{request_id}"));
                self.resume_active_task();
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
                self.push_entry_with_format(
                    EntryKind::System,
                    EntryFormat::Markdown,
                    format!("PLAN r{revision}\n{}", sanitize_terminal_text(markdown)),
                    markdown.lines().count() > 12,
                );
                self.wait_active_task("Waiting for plan review".into());
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
                self.resume_active_task();
            }
            AppEvent::WarningRaised { message } => {
                self.push_entry(
                    EntryKind::System,
                    format!("WARNING: {}", sanitize_terminal_text(message)),
                    false,
                );
            }
            AppEvent::SecurityStatusChanged { state } => {
                self.security = match state {
                    crate::app::SecurityStatus::UnattestedDevelopment => "UNATTESTED DEV",
                    crate::app::SecurityStatus::Verified => "SECURE",
                    crate::app::SecurityStatus::Failed => "SECURITY FAILED",
                };
            }
            AppEvent::TurnCompleted { turn_id } => {
                self.running = false;
                self.status = "Task complete".into();
                if let Some(index) = self.task_index(turn_id)
                    && let Some(task) = self.entries[index].task.as_mut()
                {
                    task.finish(TaskPhase::Completed, None);
                }
                self.transition_ticks = if self.options.animation { 4 } else { 0 };
            }
            AppEvent::TurnCancelled { turn_id } => {
                self.running = false;
                self.status = "Cancelled".into();
                if let Some(index) = self.task_index(turn_id)
                    && let Some(task) = self.entries[index].task.as_mut()
                {
                    task.finish(TaskPhase::Cancelled, None);
                }
                self.transition_ticks = if self.options.animation { 2 } else { 0 };
            }
            AppEvent::ErrorRaised { turn_id, message } => {
                self.running = false;
                self.status = "Error".into();
                let detail = sanitize_terminal_text(message);
                let message = user_facing_provider_error(&detail);
                let handled = turn_id
                    .as_ref()
                    .and_then(|turn_id| self.task_index(turn_id));
                if let Some(index) = handled {
                    if let Some(task) = self.entries[index].task.as_mut() {
                        task.fail(message.clone(), (message != detail).then_some(detail));
                    }
                } else {
                    self.push_entry(EntryKind::Error, message, false);
                }
                self.transition_ticks = if self.options.animation { 2 } else { 0 };
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
            if let Some(task) = &entry.task {
                lines.extend(self.task_lines(index, entry, task, width, theme));
                continue;
            }
            let (marker, color, label) = match entry.kind {
                EntryKind::User => (self.glyph("›", ">"), theme.cherry_bright, "YOU"),
                EntryKind::Assistant | EntryKind::Task => {
                    (self.glyph("◆", "*"), theme.cherry, "AXIOM")
                }
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
        }
        lines
    }

    fn task_lines(
        &self,
        index: usize,
        entry: &Entry,
        task: &TaskRunView,
        width: u16,
        theme: Theme,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let transitioning = self.transition_ticks > 0
            && self
                .entries
                .iter()
                .rev()
                .filter_map(|entry| entry.task.as_ref())
                .find(|candidate| candidate.phase.is_terminal())
                .is_some_and(|candidate| candidate.turn_id == task.turn_id);
        let (marker, marker_color, state_label) = match task.phase {
            TaskPhase::Starting | TaskPhase::Running => {
                (self.glyph("◆", "*"), theme.cherry, "Working")
            }
            TaskPhase::Waiting => (self.glyph("◇", "o"), theme.blossom_pale, "Waiting for you"),
            TaskPhase::Completed => (
                if transitioning {
                    self.completion_glyph()
                } else {
                    self.glyph("✓", "+")
                },
                if transitioning {
                    self.blossom_color(theme, 0)
                } else {
                    theme.success
                },
                "Complete",
            ),
            TaskPhase::Failed => (
                if transitioning && self.transition_ticks == 2 {
                    self.glyph("❀", "O")
                } else {
                    self.glyph("✕", "x")
                },
                theme.error,
                "Stopped with an error",
            ),
            TaskPhase::Cancelled => (self.glyph("■", "x"), theme.warning, "Cancelled"),
        };
        let mut label_style = Style::default()
            .fg(theme.cherry)
            .add_modifier(Modifier::BOLD);
        if self.selected == Some(index) {
            label_style = label_style.fg(theme.text).bg(theme.cherry_deep);
        }
        let mut header = vec![
            Span::styled(marker, Style::default().fg(marker_color)),
            Span::raw(" "),
            Span::styled("AXIOM", label_style),
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
        match task.phase {
            TaskPhase::Starting | TaskPhase::Running | TaskPhase::Waiting => {
                for item in task.tasks.iter().take(4) {
                    let (glyph, color) = match item.status {
                        crate::app::TaskStatus::Pending => (self.glyph("○", "o"), theme.muted),
                        crate::app::TaskStatus::InProgress => {
                            (self.blossom_glyph(false), theme.cherry_bright)
                        }
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
                let completed_tool_count = task
                    .tools
                    .iter()
                    .filter(|tool| tool.phase.is_terminal())
                    .count();
                if completed_tool_count > 1 {
                    self.push_task_body(
                        &mut lines,
                        task,
                        &format!("{} {}", self.glyph("✓", "+"), task.completion_summary()),
                        Style::default().fg(theme.success),
                        body_width,
                        theme,
                        rail_row,
                    );
                    rail_row = rail_row.saturating_add(1);
                } else {
                    for tool in task.recent_completed_tools(1) {
                        let (glyph, color) = if tool.phase == ToolPhase::Completed {
                            (self.glyph("✓", "+"), theme.success)
                        } else {
                            (self.glyph("✕", "x"), theme.error)
                        };
                        self.push_task_body(
                            &mut lines,
                            task,
                            &format!("{glyph} {}", tool.completed_label()),
                            Style::default().fg(color),
                            body_width,
                            theme,
                            rail_row,
                        );
                        rail_row = rail_row.saturating_add(1);
                    }
                }
                let waiting = task.phase == TaskPhase::Waiting;
                self.push_task_body(
                    &mut lines,
                    task,
                    &format!("{} {}", self.blossom_glyph(waiting), task.activity.label()),
                    Style::default().fg(if waiting {
                        theme.blossom_pale
                    } else {
                        theme.cherry_bright
                    }),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            TaskPhase::Completed => {
                self.push_task_body(
                    &mut lines,
                    task,
                    &format!("{} {}", self.glyph("✓", "+"), task.completion_summary()),
                    Style::default().fg(theme.success),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            TaskPhase::Failed => {
                self.push_task_body(
                    &mut lines,
                    task,
                    task.error.as_deref().unwrap_or("The task failed"),
                    Style::default().fg(theme.error),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
            TaskPhase::Cancelled => {
                self.push_task_body(
                    &mut lines,
                    task,
                    "Work stopped before completion",
                    Style::default().fg(theme.warning),
                    body_width,
                    theme,
                    rail_row,
                );
                rail_row = rail_row.saturating_add(1);
            }
        }

        if !task.response.is_empty() {
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
            let rendered = task
                .response_markdown
                .borrow_mut()
                .render(
                    &task.response,
                    body_width,
                    markdown_styles(theme, self.options.color_mode),
                    self.options.ascii,
                )
                .lines
                .clone();
            for mut line in rendered {
                let mut spans = self.task_rail_spans(task, theme, rail_row);
                spans.append(&mut line.spans);
                lines.push(Line::from(spans));
                rail_row = rail_row.saturating_add(1);
            }
        }

        if !entry.collapsed {
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
                "DETAILS",
                Style::default()
                    .fg(theme.cherry_bright)
                    .add_modifier(Modifier::BOLD),
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
    fn push_task_body(
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

    fn task_rail_spans(&self, task: &TaskRunView, theme: Theme, row: u64) -> Vec<Span<'static>> {
        let color = match task.phase {
            TaskPhase::Starting | TaskPhase::Running | TaskPhase::Waiting => {
                self.blossom_color(theme, row.saturating_mul(2))
            }
            TaskPhase::Completed => theme.cherry_deep,
            TaskPhase::Failed => theme.error,
            TaskPhase::Cancelled => theme.warning,
        };
        vec![
            Span::raw("  "),
            Span::styled(self.glyph("│", "|").to_owned(), Style::default().fg(color)),
            Span::raw(" "),
        ]
    }

    fn blossom_frame(&self, offset: u64) -> (&'static str, &'static str, usize) {
        // Weighted at 160 ms/tick: rest 640 ms, quick first petal, ease into the
        // bloom, hold it for 480 ms, then wilt in two quick frames.
        const FRAMES: [(&str, &str, u64); 6] = [
            ("·", ".", 4),
            ("✦", "*", 1),
            ("✿", "o", 2),
            ("❀", "O", 3),
            ("✿", "o", 1),
            ("✦", "*", 1),
        ];
        let mut tick = self.animation_frame.saturating_add(offset) % 12;
        for (index, (unicode, ascii, duration)) in FRAMES.into_iter().enumerate() {
            if tick < duration {
                return (unicode, ascii, index);
            }
            tick -= duration;
        }
        ("·", ".", 0)
    }

    fn blossom_glyph(&self, waiting: bool) -> &'static str {
        if waiting {
            return self.glyph("❀", "O");
        }
        if !self.options.animation {
            return self.glyph("✿", "*");
        }
        let (unicode, ascii, _) = self.blossom_frame(0);
        self.glyph(unicode, ascii)
    }

    fn blossom_color(&self, theme: Theme, offset: u64) -> Color {
        if !self.options.animation {
            return theme.cherry;
        }
        match self.blossom_frame(offset).2 {
            0 => theme.cherry_deep,
            2 | 4 => theme.cherry_bright,
            3 => theme.blossom_pale,
            _ => theme.cherry,
        }
    }

    fn completion_glyph(&self) -> &'static str {
        match self.transition_ticks {
            4 => self.glyph("✦", "*"),
            3 => self.glyph("✿", "o"),
            2 => self.glyph("❀", "O"),
            _ => self.glyph("✓", "+"),
        }
    }

    fn should_animate(&self) -> bool {
        self.options.animation
            && (self.running || self.transition_ticks > 0 || self.has_background_activity())
    }

    fn tick_animation(&mut self) {
        self.animation_frame = self.animation_frame.wrapping_add(1);
        self.transition_ticks = self.transition_ticks.saturating_sub(1);
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
                    let searchable = entry
                        .task
                        .as_ref()
                        .map_or_else(|| entry.text.clone(), TaskRunView::searchable_text);
                    searchable.to_lowercase().contains(&needle).then_some(index)
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
    let mut active_turn: Option<TurnId> = None;
    let mut pending_approval: Option<(ApprovalRequest, oneshot::Sender<ApprovalResponse>)> = None;
    let mut pending_questions: Option<PendingQuestions> = None;
    let mut animation_ticks = tokio::time::interval(ANIMATION_TICK);
    animation_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    assert!(
        !(cfg!(debug_assertions) && env::var("AXIOMCLI_TEST_TUI_PANIC").as_deref() == Ok("1")),
        "requested terminal restoration probe"
    );

    loop {
        terminal.draw(|frame| render(frame, &state))?;
        tokio::select! {
            _ = animation_ticks.tick(), if state.should_animate() => {
                state.tick_animation();
            }
            maybe_event = input_events.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                match event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('c')
                        {
                            if state.exit_armed {
                                if let Some(token) = &cancellation {
                                    token.cancel();
                                }
                                if let Some(turn_id) = active_turn.take() {
                                    let cancelled = runtime
                                        .dispatch(AppCommand::CancelTurn {
                                            session_id: session_id.clone(),
                                            turn_id,
                                        })
                                        .await?;
                                    if let Some(store) = &store {
                                        store.append_all(&cancelled)?;
                                    }
                                }
                                break;
                            }
                            state.exit_armed = true;
                        } else if pending_questions.is_some() {
                            state.exit_armed = false;
                            match key.code {
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
                            state.exit_armed = false;
                            match key.code {
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
                            state.exit_armed = false;
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
                            state.exit_armed = false;
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
                        } else {
                            state.exit_armed = false;
                            match key.code {
                                KeyCode::Esc => {
                                    state.focus = Focus::None;
                                    state.status = "Focus cleared".into();
                                }
                                _ if is_search_shortcut(&key) => {
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
                                    active_turn = Some(turn_id.clone());
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
                    Event::Mouse(mouse) => {
                        state.exit_armed = false;
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                state.follow_output = false;
                                state.scroll = state.scroll.saturating_sub(3);
                            }
                            MouseEventKind::ScrollDown => {
                                state.scroll = state.scroll.saturating_add(3);
                            }
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
                                    Some(Focus::None) | None => {}
                                }
                            }
                            _ => {}
                        }
                    }
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
                        active_turn = None;
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
    let status_color = if state.exit_armed {
        theme.warning
    } else if state.running || state.has_background_activity() || state.transition_ticks > 0 {
        state.blossom_color(theme, 0)
    } else {
        theme.success
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
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                if state.exit_armed {
                    state.glyph("!", "!")
                } else if state.running {
                    state.blossom_glyph(state.waiting_for_user())
                } else if state.has_background_activity() {
                    state.blossom_glyph(false)
                } else if state.transition_ticks > 0 {
                    state.completion_glyph()
                } else {
                    state.glyph("○", "o")
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
        ("Ctrl+F", "search"),
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
             Enter: send or show/hide selected task details\n\
             Ctrl+F: search transcript; n/N: next/previous match\n\
             v: open selected entry in this terminal-viewport viewer\n\
             End: follow streaming output\n\
             Ctrl+C: press twice to exit\n\
             Esc: clear focus or close the current overlay\n\
             Mouse wheel: scroll; click: focus transcript"
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

fn format_elapsed(duration: Duration) -> String {
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

fn user_facing_provider_error(message: &str) -> String {
    let normalized = message.to_ascii_lowercase();
    let payment_required = normalized.contains("http 402")
        || normalized.contains("402 payment required")
        || normalized.contains("payment required");
    let insufficient_credit = normalized.contains("insufficient_credit")
        || normalized.contains("insufficient credit")
        || normalized.contains("out of credit")
        || normalized.contains("top up your axiom balance");
    if payment_required && insufficient_credit {
        "Out of credits\nVisit axiom.stream to top up your balance.".into()
    } else {
        message.to_owned()
    }
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
    fn ctrl_f_is_search_and_slash_is_reserved_for_commands() {
        assert!(is_search_shortcut(&KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::CONTROL,
        )));
        assert!(!is_search_shortcut(&KeyEvent::new(
            KeyCode::Char('/'),
            KeyModifiers::NONE,
        )));
        assert!(!is_search_shortcut(&KeyEvent::new(
            KeyCode::Char('f'),
            KeyModifiers::NONE,
        )));
    }

    #[test]
    fn armed_exit_displays_the_double_ctrl_c_confirmation() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.exit_armed = true;

        assert!(render_symbols(100, 30, &state).contains("Press Ctrl+C again to exit"));
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
    fn insufficient_credit_failure_has_a_specific_top_up_message() {
        let mut state = TuiState::with_options(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        );
        let turn = TurnId::new();
        state.apply(&AppEvent::PromptAccepted {
            turn_id: turn.clone(),
            text: "inspect the repository".into(),
        });
        state.apply(&AppEvent::TurnStarted {
            turn_id: turn.clone(),
        });
        let provider_detail = "provider error: InvalidRequest HTTP 402 Payment Required: \
            {\"error\":{\"message\":\"out of credit – top up your Axiom balance\",\
            \"type\":\"insufficient_credit\"}}";
        state.apply(&AppEvent::ErrorRaised {
            turn_id: Some(turn.clone()),
            message: provider_detail.into(),
        });

        let folded = render_symbols(100, 30, &state);
        assert!(folded.contains("Out of credits"));
        assert!(folded.contains("Visit axiom.stream to top up your balance."));
        assert!(!folded.contains("insufficient_credit"));
        let task = state.entries[state.task_index(&turn).expect("task")]
            .task
            .as_ref()
            .expect("structured task");
        assert!(
            task.error_detail
                .as_deref()
                .is_some_and(|detail| detail.contains("insufficient_credit"))
        );

        let unrelated = "provider error: HTTP 402 Payment Required: card authorization failed";
        assert_eq!(user_facing_provider_error(unrelated), unrelated);
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
        assert!(state.entries.iter().any(|entry| {
            entry
                .task
                .as_ref()
                .is_some_and(|task| task.detail_text().contains("diff:"))
        }));
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
    fn task_events_update_one_live_card_and_keep_raw_details_folded() {
        let mut state = TuiState::with_options(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        );
        let turn = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: turn.clone(),
                text: "inspect the repository".into(),
            },
            AppEvent::TurnStarted {
                turn_id: turn.clone(),
            },
            AppEvent::ReasoningDelta {
                turn_id: turn.clone(),
                text: "private scratch reasoning".into(),
            },
            AppEvent::ToolProposed {
                turn_id: turn.clone(),
                call_id: "read-1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "README.md"}),
            },
            AppEvent::ToolStarted {
                call_id: "read-1".into(),
                name: "read_file".into(),
            },
            AppEvent::ToolOutput {
                call_id: "read-1".into(),
                content: "raw README contents".into(),
                truncated: false,
            },
            AppEvent::DiffAvailable {
                call_id: "read-1".into(),
                diff: "-old\n+new".into(),
                truncated: false,
            },
            AppEvent::ToolCompleted {
                call_id: "read-1".into(),
                success: true,
            },
        ] {
            state.apply(&event);
        }

        let task_entries = state
            .entries
            .iter()
            .filter(|entry| entry.kind == EntryKind::Task)
            .collect::<Vec<_>>();
        assert_eq!(task_entries.len(), 1);
        let task = task_entries[0].task.as_ref().expect("structured task");
        assert_eq!(task.tools.len(), 1);
        assert_eq!(task.tools[0].output, "raw README contents");
        assert_eq!(task.tools[0].diff, "-old\n+new");

        let folded = render_symbols(100, 30, &state);
        assert!(folded.contains("Read files"));
        assert!(!folded.contains("private scratch reasoning"));
        assert!(!folded.contains("raw README contents"));

        state.selected = state.task_index(&turn);
        state.toggle_selected();
        let expanded = render_symbols(100, 40, &state);
        assert!(expanded.contains("private scratch reasoning"));
        assert!(expanded.contains("raw README contents"));
    }

    #[test]
    fn blossom_cycle_uses_weighted_bloom_and_fast_wilt_timing() {
        let mut state = TuiState::with_options(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: true,
            },
        );
        let frames = (0..12)
            .map(|tick| {
                state.animation_frame = tick;
                state.blossom_glyph(false)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            frames,
            vec!["·", "·", "·", "·", "✦", "✿", "✿", "❀", "❀", "❀", "✿", "✦"]
        );
        for tick in 0..12 {
            state.animation_frame = tick;
            assert_eq!(state.blossom_glyph(true), "❀");
        }
        state.options.animation = false;
        assert_eq!(state.blossom_glyph(false), "✿");
    }

    #[test]
    fn assistant_markdown_is_rendered_but_user_text_stays_literal() {
        let mut state = TuiState::with_options(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        );
        state.push_entry(
            EntryKind::User,
            "please keep **these markers**".into(),
            false,
        );
        state.push_entry(
            EntryKind::Assistant,
            "## Result\n\nThe **markers disappear**, while `code` is highlighted.".into(),
            false,
        );
        let lines = state.transcript_lines(72);
        let plain = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(plain.contains("please keep **these markers**"));
        assert!(plain.contains("The markers disappear, while code is highlighted."));
        assert!(!plain.contains("## Result"));
        let code_span = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content == "code")
            .expect("inline code span");
        assert_eq!(
            code_span.style.bg,
            Some(Theme::for_mode(ColorMode::TrueColor).surface)
        );
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
        state.apply(&AppEvent::TurnStarted {
            turn_id: turn.clone(),
        });
        state.apply(&AppEvent::ToolProposed {
            turn_id: turn.clone(),
            call_id: "tool-1".into(),
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "src"}),
        });
        state.apply(&AppEvent::ToolStarted {
            call_id: "tool-1".into(),
            name: "read_file".into(),
        });
        state.apply(&AppEvent::ToolOutput {
            call_id: "tool-1".into(),
            content: "src/main.rs\nsrc/lib.rs".into(),
            truncated: false,
        });
        state.apply(&AppEvent::ToolCompleted {
            call_id: "tool-1".into(),
            success: true,
        });
        state.apply(&AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: "I’ll inspect the relevant files.".into(),
        });
        state.apply(&AppEvent::TurnCompleted { turn_id: turn });
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
                                "i": cell.modifier.contains(Modifier::ITALIC),
                                "u": cell.modifier.contains(Modifier::UNDERLINED),
                                "x": cell.modifier.contains(Modifier::CROSSED_OUT),
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
            PathBuf::from("/work/axiomai"),
            "glm-5-2".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        )
    }

    fn bloom_preview(frame: u64) -> TuiState {
        let mut state = preview_state();
        state.options.animation = true;
        state.animation_frame = frame;
        let turn = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: turn.clone(),
                text: "inspect the renderer".into(),
            },
            AppEvent::TurnStarted { turn_id: turn },
            AppEvent::ProgressUpdated {
                message: "Inspecting the renderer".into(),
                completed: None,
                total: None,
            },
        ] {
            state.apply(&event);
        }
        state
    }

    fn preview_scenarios() -> Vec<(&'static str, u16, u16, TuiState)> {
        let mut conversation = preview_state();
        let conversation_turn = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: conversation_turn.clone(),
                text: "hello axiom whats up".into(),
            },
            AppEvent::TurnStarted {
                turn_id: conversation_turn.clone(),
            },
            AppEvent::ReasoningDelta {
                turn_id: conversation_turn.clone(),
                text: "The user is greeting me casually; answer warmly and concisely.".into(),
            },
            AppEvent::TextDelta {
                turn_id: conversation_turn.clone(),
                text: "Hey! 👋 I’m here in your `axiomai` workspace and ready to work. What do you want to build?".into(),
            },
            AppEvent::TurnCompleted {
                turn_id: conversation_turn,
            },
        ] {
            conversation.apply(&event);
        }

        let mut working = preview_state();
        working.options.animation = true;
        working.animation_frame = 7;
        let working_turn = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: working_turn.clone(),
                text: "run the release verification".into(),
            },
            AppEvent::TurnStarted {
                turn_id: working_turn.clone(),
            },
            AppEvent::TaskListUpdated {
                items: vec![
                    crate::app::TaskItem {
                        id: "signatures".into(),
                        title: "Verify signatures".into(),
                        status: crate::app::TaskStatus::Completed,
                    },
                    crate::app::TaskItem {
                        id: "manifests".into(),
                        title: "Check release manifests".into(),
                        status: crate::app::TaskStatus::Completed,
                    },
                    crate::app::TaskItem {
                        id: "package".into(),
                        title: "Package the tarball".into(),
                        status: crate::app::TaskStatus::InProgress,
                    },
                ],
            },
            AppEvent::ToolProposed {
                turn_id: working_turn.clone(),
                call_id: "read-release".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "RELEASE.md"}),
            },
            AppEvent::ToolStarted {
                call_id: "read-release".into(),
                name: "read_file".into(),
            },
            AppEvent::ToolOutput {
                call_id: "read-release".into(),
                content: "signatures: valid\nmanifests: valid".into(),
                truncated: false,
            },
            AppEvent::ToolCompleted {
                call_id: "read-release".into(),
                success: true,
            },
            AppEvent::ToolProposed {
                turn_id: working_turn.clone(),
                call_id: "package".into(),
                name: "shell".into(),
                arguments: serde_json::json!({"command": "scripts/package-release"}),
            },
            AppEvent::ToolStarted {
                call_id: "package".into(),
                name: "shell".into(),
            },
            AppEvent::ProgressUpdated {
                message: "Packaging the release tarball".into(),
                completed: None,
                total: None,
            },
        ] {
            working.apply(&event);
        }

        let mut waiting = preview_state();
        waiting.options.animation = true;
        waiting.animation_frame = 9;
        let waiting_turn = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: waiting_turn.clone(),
                text: "update the release manifest".into(),
            },
            AppEvent::TurnStarted {
                turn_id: waiting_turn.clone(),
            },
            AppEvent::ToolProposed {
                turn_id: waiting_turn,
                call_id: "write-manifest".into(),
                name: "apply_patch".into(),
                arguments: serde_json::json!({"path": "RELEASE.md"}),
            },
            AppEvent::PermissionRequired {
                request_id: "permission-1".into(),
                explanation: "Edit RELEASE.md".into(),
            },
        ] {
            waiting.apply(&event);
        }

        let mut details = preview_state();
        let details_turn = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: details_turn.clone(),
                text: "fix the renderer regression".into(),
            },
            AppEvent::TurnStarted {
                turn_id: details_turn.clone(),
            },
            AppEvent::ReasoningDelta {
                turn_id: details_turn.clone(),
                text: "The regression begins in the task reducer. I should inspect the call identity path before editing.".into(),
            },
            AppEvent::ToolProposed {
                turn_id: details_turn.clone(),
                call_id: "search-reducer".into(),
                name: "search_files".into(),
                arguments: serde_json::json!({"query": "call_id", "path": "axiomcli/src/tui"}),
            },
            AppEvent::ToolStarted {
                call_id: "search-reducer".into(),
                name: "search_files".into(),
            },
            AppEvent::ToolOutput {
                call_id: "search-reducer".into(),
                content: "axiomcli/src/tui.rs:511\naxiomcli/src/tui/task.rs:142".into(),
                truncated: false,
            },
            AppEvent::ToolCompleted {
                call_id: "search-reducer".into(),
                success: true,
            },
            AppEvent::ToolProposed {
                turn_id: details_turn.clone(),
                call_id: "patch-reducer".into(),
                name: "apply_patch".into(),
                arguments: serde_json::json!({"path": "axiomcli/src/tui.rs"}),
            },
            AppEvent::ToolStarted {
                call_id: "patch-reducer".into(),
                name: "apply_patch".into(),
            },
            AppEvent::DiffAvailable {
                call_id: "patch-reducer".into(),
                diff: "- push_entry(tool_output)\n+ update_tool(call_id)".into(),
                truncated: false,
            },
            AppEvent::ToolCompleted {
                call_id: "patch-reducer".into(),
                success: true,
            },
            AppEvent::TextDelta {
                turn_id: details_turn.clone(),
                text: "The reducer now updates each tool call in place.".into(),
            },
            AppEvent::TurnCompleted {
                turn_id: details_turn.clone(),
            },
        ] {
            details.apply(&event);
        }
        details.focus = Focus::Transcript;
        details.selected = details.task_index(&details_turn);
        details.toggle_selected();

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

        let mut out_of_credits = preview_state();
        let credits_turn = TurnId::new();
        for event in [
            AppEvent::PromptAccepted {
                turn_id: credits_turn.clone(),
                text: "what is this repository about?".into(),
            },
            AppEvent::TurnStarted {
                turn_id: credits_turn.clone(),
            },
            AppEvent::ErrorRaised {
                turn_id: Some(credits_turn),
                message: "provider error: InvalidRequest HTTP 402 Payment Required: \
                    {\"error\":{\"message\":\"out of credit – top up your Axiom balance\",\
                    \"type\":\"insufficient_credit\"}}"
                    .into(),
            },
        ] {
            out_of_credits.apply(&event);
        }

        let mut overlay = preview_state();
        overlay.push_entry(EntryKind::User, "help".into(), false);
        overlay.overlay = Some(Overlay::Help);

        let narrow = preview_state();
        let minimum = preview_state();

        let mut markdown = preview_state();
        markdown.entries.clear();
        markdown.push_entry(
            EntryKind::Assistant,
            "# Renderer ready\n\nAxiom now renders **strong**, *emphasis*, ~~removed text~~, and \
             [`styled links`](https://example.com) without exposing Markdown punctuation.\n\n\
             - [x] Preserve the raw response\n\
             - [x] Wrap styled Unicode like blossoms 🌸 correctly\n\
             - [ ] Add optional Mermaid rendering later\n\n\
             > [!NOTE]\n\
             > Security and tool output remain literal.\n\n\
             | Surface | Behavior |\n\
             |:--|:--|\n\
             | Assistant | Markdown |\n\
             | Tools | Literal |\n\n\
             ```rust\n\
             fn blossom(private: bool) -> &'static str {\n\
                 if private { \"encrypted 🌸\" } else { \"blocked\" }\n\
             }\n\
             ```"
            .into(),
            false,
        );

        let mut markdown_narrow = preview_state();
        markdown_narrow.entries.clear();
        markdown_narrow.push_entry(
            EntryKind::Assistant,
            "## Narrow terminal\n\nStyled prose wraps without losing **weight** or its \
             [destination](https://example.com/a/long/path).\n\n\
             | Name | State | Owner | Scope | Mode |\n\
             |---|---|---|---|---|\n\
             | renderer | ready | axiom | terminal | safe |\n\n\
             ```sh\naxiomcli tui --cwd /a/very/long/project/path/that/is/clipped\n```"
                .into(),
            false,
        );

        vec![
            ("conversation", 100, 30, conversation),
            ("working", 100, 30, working),
            ("waiting", 100, 30, waiting),
            ("details", 100, 42, details),
            ("bloom-rest", 84, 20, bloom_preview(0)),
            ("bloom-open", 84, 20, bloom_preview(5)),
            ("bloom-full", 84, 20, bloom_preview(7)),
            ("bloom-wilt", 84, 20, bloom_preview(11)),
            ("browsing", 100, 30, browsing),
            ("out-of-credits", 100, 30, out_of_credits),
            ("overlay", 100, 30, overlay),
            ("narrow", 54, 16, narrow),
            ("minimum", 42, 10, minimum),
            ("markdown", 100, 42, markdown),
            ("markdown-narrow", 60, 30, markdown_narrow),
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
