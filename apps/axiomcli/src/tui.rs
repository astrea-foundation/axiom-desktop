use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    env,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use axiom_secure_client::SecurityEvidence;
use crossterm::{
    Command,
    event::{
        DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
        KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt as _;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Wrap},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    AxiomError, Result,
    agent::{
        APP_EVENT_QUEUE_CAPACITY, CompactionResult, QuestionHandler, SecurityVerification,
        TurnContext, TurnRunner,
    },
    app::{
        AppCommand, AppEvent, Origin, PermissionProfile, QuestionRequest, Runtime, SessionId,
        ThinkingLevel, TurnId,
    },
    auth::{AccountStatus, AuthManager, BrowserLogin, ValidationStatus},
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse, describe_effect},
    session::{
        SessionStore, SessionSummary, model_for_resume, permission_for_resume, thinking_for_resume,
    },
    slash::{self, SlashCommand},
};

mod composer;
mod markdown;
mod task;

use composer::ComposerBuffer;
use markdown::{MarkdownStyles, StreamingMarkdownRenderer};
use task::{TaskActivity, TaskPhase, TaskRunView, TaskTimelineItem, ToolPhase};

/// Every region of the shell is inset by the same number of columns so text,
/// dividers and the composer all line up on one left edge.
const GUTTER: u16 = 2;
/// Columns reserved for the role marker; entry bodies are indented to match so
/// wrapped continuations stay under the first line instead of falling to zero.
const BODY_INDENT: usize = 2;
/// Smallest terminal the full layout is designed for.
const MIN_WIDTH: u16 = 42;
const MIN_HEIGHT: u16 = 10;
const COMPOSER_MAX_CONTENT_ROWS: u16 = 8;
const ANIMATION_TICK: Duration = Duration::from_millis(160);
const SPLASH_MESSAGES: &[&str] = &[
    "Think freely.",
    "Off the record.",
    "Nobody's listening.",
    "A quiet place to think.",
    "Speak freely.",
    "Your words stay yours.",
];
const STARTUP_BLOSSOM_ART: [&str; 11] = [
    "   ⢀⡀⢀⣀       ⣀⡀       ",
    " ⢀⠔⠁ ⠢⢀⠈⠑⢶⠉⠉⢩⠁ ⡄⠑⢄     ",
    " ⢸  ⠐⠢⢀⠡⡐⡘⡀⠈⠸⣤⣾⡿⠲⠈⡆    ",
    "  ⢆ ⢀⡒⠤⣑⡑⣰⢡⡄⡣⢺⠛⡁⢀⡴⠒⠠⢄  ",
    " ⢀⠔⢉ ⠤⠭⢐⡻⡟⣋⡽⢱⢷⠬⢐⠭⢐⣂⡀ ⠉⢆",
    " ⠃ ⢀⠄⠒ ⣈⠘⢧⣿⣴⣃⣘⡾⠦⠭⠤⠄⣀  ⢸",
    "⢸⡀ ⠁⠠⠂⠁⡠⠂⢡⢪⠛⠚⡝⠫⠑⠈⠑⢄⣀⣀⠴⠋",
    " ⠱⡀   ⠈  ⢃⠃ ⡇⠘⡀⠡ ⠐⠄⡇   ",
    "  ⠈⠑⠒⠤⠤⠤⠤⢾⡀   ⠇ ⠃  ⡇   ",
    "          ⠙⠂⢤⢀⡀   ⡸    ",
    "               ⠈⠉⠉     ",
];
const STARTUP_COMPACT_BLOSSOM_ART: [&str; 8] = [
    "  ⡠⢀⠤⣀⣀⣀⣀⡠⠤⣀    ",
    " ⡎ ⣬⡑⣄⡽ ⢼⣀⣾⣬⢧   ",
    " ⢵⣀⣰⣬⣞⣺⣧⣲⣿⣋⣠⠾⠠⣄ ",
    "⢠⠞⠡⢐⡛⠽⣿⡽⣻⢿⣛⣿⠿⠆⠈⢳",
    "⣴ ⠚⡠⠂⡣⢛⣿⠿⡿⡯⠽⢍⣚⣀⡼",
    "⠈⢆⡀ ⠈ ⢻⠃⠿⢹⡸⡀⠢⡏⠁ ",
    "  ⠈⠙⠛⠛⠛⣦⢀⠘⠁⠁⢠⡇  ",
    "        ⠈⠉⠐⠒⠊   ",
];
const STARTUP_ASCII_ART: [&str; 7] = [
    "        .  *  .",
    "     *  . * .  *",
    "   . *  * @ *  * .",
    "     *  . * .  *",
    "        `\\|/`",
    "          |",
    "          |",
];

fn pick_splash_message() -> &'static str {
    let entropy = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let count = u128::try_from(SPLASH_MESSAGES.len()).unwrap_or(1);
    let index = usize::try_from(entropy % count).unwrap_or_default();
    SPLASH_MESSAGES[index]
}

#[derive(Clone, Copy, Debug)]
struct SetAxiomTerminalBackground(Option<&'static str>);

impl Command for SetAxiomTerminalBackground {
    fn write_ansi(&self, formatter: &mut impl std::fmt::Write) -> std::fmt::Result {
        if let Some(color) = self.0 {
            write!(formatter, "\x1b]11;{color}\x1b\\")?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        use std::io::Write as _;
        if let Some(color) = self.0 {
            write!(io::stdout(), "\x1b]11;{color}\x1b\\")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct ResetAxiomTerminalBackground(bool);

impl Command for ResetAxiomTerminalBackground {
    fn write_ansi(&self, formatter: &mut impl std::fmt::Write) -> std::fmt::Result {
        if self.0 {
            formatter.write_str("\x1b]111\x1b\\")?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        use std::io::Write as _;
        if self.0 {
            io::stdout().write_all(b"\x1b]111\x1b\\")?;
        }
        Ok(())
    }
}

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

    const fn terminal_background(self) -> Option<&'static str> {
        match self {
            Self::TrueColor | Self::Ansi256 => Some("#101013"),
            Self::Ansi16 => Some("#000000"),
            Self::NoColor => None,
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

fn uses_deterministic_test_runner() -> bool {
    cfg!(debug_assertions)
        && env::var("AXIOMCLI_TEST_RUNNER").is_ok_and(|value| !value.trim().is_empty())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Overlay {
    Entry(usize),
    Help,
    Permissions {
        tab: PermissionTab,
        selected: usize,
    },
    ModelPicker {
        query: String,
        selected: usize,
    },
    ResumePicker {
        query: String,
        selected: usize,
        sessions: Vec<SessionSummary>,
    },
    DeletePicker {
        query: String,
        selected: usize,
        sessions: Vec<SessionSummary>,
        marked: BTreeSet<String>,
        confirming: bool,
    },
    Security(SecurityView),
    SaveAttestation(SaveAttestationPicker),
    Auth(AuthOverlay),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SavePickerFocus {
    Directories,
    #[default]
    Filename,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SaveAttestationPicker {
    directory: PathBuf,
    filename: String,
    selected: usize,
    focus: SavePickerFocus,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PermissionTab {
    #[default]
    Tools,
    Crypto,
}

impl PermissionTab {
    const fn other(self) -> Self {
        match self {
            Self::Tools => Self::Crypto,
            Self::Crypto => Self::Tools,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SecurityView {
    #[default]
    Summary,
    Workload,
    Raw,
}

impl SecurityView {
    const fn next(self) -> Self {
        match self {
            Self::Summary => Self::Workload,
            Self::Workload => Self::Raw,
            Self::Raw => Self::Summary,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AuthPasteKind {
    ApiKey,
    CompletionCode,
}

#[derive(Clone, PartialEq, Eq)]
struct SecretInput(String);

impl std::fmt::Debug for SecretInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("SecretInput")
            .field(&format_args!(
                "[REDACTED; {} chars]",
                self.0.chars().count()
            ))
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AuthOverlay {
    Menu {
        selected: usize,
        message: Option<String>,
    },
    Starting,
    Browser {
        user_code: String,
        authorization_url: String,
        browser_opened: bool,
    },
    Paste {
        kind: AuthPasteKind,
        input: SecretInput,
    },
    Checking {
        label: &'static str,
    },
    Account {
        status: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusTone {
    Neutral,
    Active,
    Waiting,
    Success,
    Cancelled,
    Error,
}

#[derive(Clone, Copy, Debug)]
pub struct TuiOptions {
    pub color_mode: ColorMode,
    pub ascii: bool,
    pub animation: bool,
}

pub struct TuiLaunch {
    pub options: TuiOptions,
    pub store: Option<SessionStore>,
    pub resume: Option<SessionId>,
    pub auth: AuthManager,
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
struct SlashCompletionCycle {
    candidates: Vec<slash::SlashCompletion>,
    index: usize,
}

#[derive(Clone, Debug)]
enum InteractionView {
    Approval(ApprovalRequest),
    Questions {
        request: QuestionRequest,
        index: usize,
    },
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct TuiState {
    cwd: PathBuf,
    model: String,
    thinking: ThinkingLevel,
    splash_message: &'static str,
    model_candidates: Vec<String>,
    model_catalog_loaded: bool,
    model_catalog_loading: bool,
    slash_completion: Option<SlashCompletionCycle>,
    slash_selection: usize,
    slash_selection_moved: bool,
    profile: PermissionProfile,
    security: &'static str,
    security_evidence: Option<SecurityEvidence>,
    input: ComposerBuffer,
    entries: Vec<Entry>,
    running: bool,
    status: String,
    status_tone: StatusTone,
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
    interaction: Option<InteractionView>,
    input_tokens: u64,
    output_tokens: u64,
    current_reported_input_tokens: u64,
    current_reported_output_tokens: u64,
    unreported_output_bytes: u64,
    unconfirmed_tokens: u64,
    animation_frame: u64,
    transition_ticks: u8,
    compacting: bool,
    has_prompt: bool,
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
            thinking: ThinkingLevel::Medium,
            splash_message: pick_splash_message(),
            model_candidates: Vec::new(),
            model_catalog_loaded: false,
            model_catalog_loading: false,
            slash_completion: None,
            slash_selection: 0,
            slash_selection_moved: false,
            profile,
            security: "NOT VERIFIED",
            security_evidence: None,
            input: ComposerBuffer::default(),
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
            status_tone: StatusTone::Success,
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
            interaction: None,
            input_tokens: 0,
            output_tokens: 0,
            current_reported_input_tokens: 0,
            current_reported_output_tokens: 0,
            unreported_output_bytes: 0,
            unconfirmed_tokens: 0,
            animation_frame: 0,
            transition_ticks: 0,
            compacting: false,
            has_prompt: false,
        }
    }

    fn open_login(&mut self, message: Option<String>) {
        self.overlay = Some(Overlay::Auth(AuthOverlay::Menu {
            selected: 0,
            message,
        }));
        self.overlay_scroll = 0;
        self.status = "Sign in to Axiom".into();
        self.status_tone = StatusTone::Waiting;
    }

    fn reset_security(&mut self) {
        self.security = "NOT VERIFIED";
        self.security_evidence = None;
    }

    fn open_security(&mut self) {
        self.overlay = Some(Overlay::Security(SecurityView::Summary));
        self.overlay_scroll = 0;
    }

    fn cycle_security_view(&mut self) {
        if let Some(Overlay::Security(view)) = &mut self.overlay {
            *view = view.next();
            self.overlay_scroll = 0;
        }
    }

    fn open_attestation_save_picker(&mut self) {
        let Some(evidence) = self.security_evidence.as_ref() else {
            self.status = "No attestation result is available to save".into();
            self.status_tone = StatusTone::Error;
            return;
        };
        self.overlay = Some(Overlay::SaveAttestation(SaveAttestationPicker {
            directory: self.cwd.clone(),
            filename: attestation_export_filename(evidence),
            selected: 0,
            focus: SavePickerFocus::Filename,
            error: None,
        }));
        self.overlay_scroll = 0;
    }

    fn close_attestation_save_picker(&mut self) {
        self.overlay = Some(Overlay::Security(SecurityView::Raw));
        self.overlay_scroll = 0;
    }

    fn toggle_attestation_save_focus(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        picker.focus = match picker.focus {
            SavePickerFocus::Directories => SavePickerFocus::Filename,
            SavePickerFocus::Filename => SavePickerFocus::Directories,
        };
        picker.error = None;
    }

    fn move_attestation_directory_selection(&mut self, amount: isize) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        if picker.focus != SavePickerFocus::Directories {
            return;
        }
        let count = attestation_picker_directories(&picker.directory).len();
        if count == 0 {
            picker.selected = 0;
            return;
        }
        picker.selected = if amount.is_negative() {
            picker.selected.saturating_sub(amount.unsigned_abs())
        } else {
            picker
                .selected
                .saturating_add(amount.unsigned_abs())
                .min(count - 1)
        };
    }

    fn open_selected_attestation_directory(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        let directories = attestation_picker_directories(&picker.directory);
        let Some((_, next_directory)) = directories.get(picker.selected) else {
            return;
        };
        picker.directory.clone_from(next_directory);
        picker.selected = 0;
        picker.error = None;
    }

    fn open_parent_attestation_directory(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        let Some(parent) = picker.directory.parent() else {
            return;
        };
        picker.directory = parent.to_path_buf();
        picker.selected = 0;
        picker.error = None;
    }

    fn push_attestation_filename(&mut self, text: &str) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        if picker.focus != SavePickerFocus::Filename {
            return;
        }
        let remaining = 255_usize.saturating_sub(picker.filename.len());
        picker
            .filename
            .push_str(&text.chars().take(remaining).collect::<String>());
        picker.error = None;
    }

    fn pop_attestation_filename(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        if picker.focus == SavePickerFocus::Filename {
            picker.filename.pop();
            picker.error = None;
        }
    }

    fn clear_attestation_filename(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        if picker.focus == SavePickerFocus::Filename {
            picker.filename.clear();
            picker.error = None;
        }
    }

    fn save_attestation_export(&mut self) {
        let Some(evidence) = self.security_evidence.as_ref() else {
            self.close_attestation_save_picker();
            self.status = "No attestation result is available to save".into();
            self.status_tone = StatusTone::Error;
            return;
        };
        let Some(Overlay::SaveAttestation(picker)) = &self.overlay else {
            return;
        };
        let directory = picker.directory.clone();
        let filename = picker.filename.clone();
        match write_attestation_export(&directory, &filename, evidence) {
            Ok(destination) => {
                self.close_attestation_save_picker();
                self.status = format!("Saved attestation to {}", destination.display());
                self.status_tone = StatusTone::Success;
            }
            Err(error) => {
                if let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay {
                    picker.error = Some(sanitize_terminal_text(&error.to_string()));
                }
                self.status = "Attestation was not saved".into();
                self.status_tone = StatusTone::Error;
            }
        }
    }

    fn auth_menu_move(&mut self, amount: isize) {
        let Some(Overlay::Auth(AuthOverlay::Menu { selected, .. })) = &mut self.overlay else {
            return;
        };
        *selected = usize::try_from(
            (isize::try_from(*selected).unwrap_or_default() + amount).rem_euclid(2),
        )
        .unwrap_or_default();
    }

    fn auth_secret_push(&mut self, text: &str) {
        if let Some(Overlay::Auth(AuthOverlay::Paste { input, .. })) = &mut self.overlay {
            let remaining = 512_usize.saturating_sub(input.0.len());
            input
                .0
                .push_str(&text.chars().take(remaining).collect::<String>());
        }
    }

    fn auth_secret_pop(&mut self) {
        if let Some(Overlay::Auth(AuthOverlay::Paste { input, .. })) = &mut self.overlay {
            input.0.pop();
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
        let cwd = self.cwd.clone();
        if !self.has_pending_user_attention()
            && let Some(task) = self.active_task_mut()
        {
            task.phase = TaskPhase::Running;
            task.activity = task.running_tool().map_or(TaskActivity::Thinking, |tool| {
                TaskActivity::Working(tool.timeline_label(&cwd))
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

    fn has_background_activity(&self) -> bool {
        self.model_catalog_loading
            || self
                .pending_attention
                .keys()
                .any(|key| key.starts_with("background:"))
    }

    fn clear_foreground_attention(&mut self) {
        self.pending_attention
            .retain(|key, _| key.starts_with("background:"));
        self.answer_input.clear();
        self.answer_prompt = None;
        self.interaction = None;
    }

    fn clear_permission_interaction(&mut self, request_id: &str) {
        if matches!(
            &self.interaction,
            Some(InteractionView::Approval(request)) if request.request_id == request_id
        ) {
            self.interaction = None;
        }
    }

    fn clear_question_interaction(&mut self, request_id: &str) {
        if matches!(
            &self.interaction,
            Some(InteractionView::Questions { request, .. }) if request.request_id == request_id
        ) {
            self.answer_input.clear();
            self.answer_prompt = None;
            self.interaction = None;
        }
    }

    // Retained for a future token-usage surface; collection remains live even
    // though the current TUI intentionally does not render the counter.
    #[allow(dead_code)]
    fn displayed_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.unconfirmed_tokens)
            .saturating_add(estimate_streamed_tokens(self.unreported_output_bytes))
    }

    fn record_streamed_output(&mut self, text: &str) {
        self.unreported_output_bytes = self
            .unreported_output_bytes
            .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
    }

    fn begin_provider_request(&mut self) {
        self.unconfirmed_tokens = self
            .unconfirmed_tokens
            .saturating_add(estimate_streamed_tokens(self.unreported_output_bytes));
        self.unreported_output_bytes = 0;
        self.current_reported_input_tokens = 0;
        self.current_reported_output_tokens = 0;
    }

    fn apply_authoritative_usage(&mut self, input_tokens: u64, output_tokens: u64) {
        self.input_tokens = self
            .input_tokens
            .saturating_sub(self.current_reported_input_tokens)
            .saturating_add(input_tokens);
        self.output_tokens = self
            .output_tokens
            .saturating_sub(self.current_reported_output_tokens)
            .saturating_add(output_tokens);
        self.current_reported_input_tokens = input_tokens;
        self.current_reported_output_tokens = output_tokens;
        self.unreported_output_bytes = 0;
    }

    fn apply(&mut self, event: &AppEvent) {
        match event {
            AppEvent::PromptAccepted { turn_id, text } => {
                self.has_prompt = true;
                self.push_entry(EntryKind::User, sanitize_terminal_text(text), false);
                self.push_task(turn_id.clone(), sanitize_terminal_text(text));
            }
            AppEvent::TurnStarted { turn_id } => {
                self.running = true;
                self.status = "Working…".into();
                self.status_tone = StatusTone::Active;
                let index = self.ensure_task(turn_id);
                if let Some(task) = self.entries[index].task.as_mut() {
                    task.phase = TaskPhase::Running;
                    task.activity = TaskActivity::Thinking;
                }
            }
            AppEvent::TextDelta { turn_id, text } => {
                self.record_streamed_output(text);
                let text = sanitize_terminal_text(text);
                if let Some(index) = self.task_index(turn_id) {
                    if let Some(task) = self.entries[index].task.as_mut() {
                        task.append_response(&text);
                        task.activity = TaskActivity::Responding;
                    }
                } else {
                    self.append_stream(EntryKind::Assistant, &text);
                }
                self.status = "Responding…".into();
                self.status_tone = StatusTone::Active;
            }
            AppEvent::ReasoningDelta { turn_id, text } => {
                self.record_streamed_output(text);
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
                self.status_tone = StatusTone::Active;
            }
            AppEvent::UsageUpdated {
                input_tokens,
                output_tokens,
            } => {
                self.apply_authoritative_usage(*input_tokens, *output_tokens);
            }
            AppEvent::ToolProposed {
                turn_id,
                call_id,
                name,
                arguments,
            } => {
                let cwd = self.cwd.clone();
                let mut activity = None;
                let index = self.ensure_task(turn_id);
                if let Some(task) = self.entries[index].task.as_mut() {
                    let label = task
                        .ensure_tool(call_id, name, arguments.clone())
                        .timeline_label(&cwd);
                    task.activity = TaskActivity::Working(label.clone());
                    activity = Some(label);
                }
                self.status = activity.unwrap_or_else(|| format!("Preparing {name}"));
                self.status_tone = StatusTone::Active;
            }
            AppEvent::ToolStarted { call_id, name } => {
                let cwd = self.cwd.clone();
                let mut activity = None;
                if let Some(task) = self.task_for_call_mut(call_id) {
                    let tool = task.ensure_tool(call_id, name, serde_json::Value::Null);
                    tool.phase = ToolPhase::Running;
                    let label = tool.timeline_label(&cwd);
                    task.activity = TaskActivity::Working(label.clone());
                    activity = Some(label);
                } else if let Some(task) = self.active_task_mut() {
                    let tool = task.ensure_tool(call_id, name, serde_json::Value::Null);
                    tool.phase = ToolPhase::Running;
                    let label = tool.timeline_label(&cwd);
                    task.activity = TaskActivity::Working(label.clone());
                    activity = Some(label);
                } else {
                    self.push_entry(
                        EntryKind::Tool,
                        format!("Running {name} · {call_id}"),
                        false,
                    );
                }
                self.status = activity.unwrap_or_else(|| format!("Running {name}"));
                self.status_tone = StatusTone::Active;
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
                let cwd = self.cwd.clone();
                let mut completed_activity = None;
                if let Some(task) = self.task_for_call_mut(call_id) {
                    if let Some(tool) = task.tool_mut(call_id) {
                        tool.phase = if *success {
                            ToolPhase::Completed
                        } else {
                            ToolPhase::Failed
                        };
                        completed_activity = Some(tool.timeline_label(&cwd));
                    }
                    if task.phase != TaskPhase::Waiting {
                        task.activity =
                            task.running_tool().map_or(TaskActivity::Thinking, |tool| {
                                TaskActivity::Working(tool.timeline_label(&cwd))
                            });
                    }
                }
                self.status = completed_activity.unwrap_or_else(|| {
                    if *success {
                        "Tool completed".into()
                    } else {
                        "Tool failed".into()
                    }
                });
                self.status_tone = if *success {
                    StatusTone::Active
                } else {
                    StatusTone::Error
                };
            }
            AppEvent::PermissionRequired {
                request_id,
                explanation,
                ..
            } => {
                self.pending_attention
                    .insert(format!("permission:{request_id}"), explanation.clone());
                self.status = format!("Permission required: {explanation}");
                self.status_tone = StatusTone::Waiting;
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
                self.status_tone = StatusTone::Active;
                self.clear_permission_interaction(request_id);
                self.resume_active_task();
            }
            AppEvent::ProviderStatusChanged { connected, detail } => {
                if *connected {
                    self.begin_provider_request();
                } else {
                    self.status =
                        format!("Provider unavailable: {}", sanitize_terminal_text(detail));
                    self.status_tone = StatusTone::Error;
                }
            }
            AppEvent::PermissionProfileChanged { profile } => {
                self.profile = *profile;
                self.status = format!("Tools permission changed to {}", profile.label());
            }
            AppEvent::ModelChanged { model } => {
                self.model.clone_from(model);
                self.security_evidence = None;
                self.status = format!("Model changed to {model}");
            }
            AppEvent::ThinkingLevelChanged { level } => {
                self.thinking = *level;
                self.status = format!("Thinking set to {level}");
            }
            AppEvent::ContextCompacted {
                messages_before, ..
            } => {
                self.running = false;
                self.compacting = false;
                self.status = "Context compacted".into();
                self.status_tone = StatusTone::Success;
                self.push_entry(
                    EntryKind::System,
                    format!(
                        "CONTEXT COMPACTED\nReplaced {messages_before} conversation message(s) with a successor summary."
                    ),
                    false,
                );
            }
            AppEvent::ProgressUpdated { message, .. } => {
                let message = sanitize_terminal_text(message);
                if let Some(task) = self.active_task_mut()
                    && task.phase != TaskPhase::Waiting
                {
                    task.activity = TaskActivity::Working(message.clone());
                }
                self.status = message;
                self.status_tone = StatusTone::Active;
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
                ..
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
                self.status_tone = StatusTone::Waiting;
            }
            AppEvent::QuestionAnswered { .. } => {
                self.resume_active_task();
                self.status_tone = StatusTone::Active;
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
                self.wait_active_task("Waiting for your answer".into());
                self.status_tone = StatusTone::Waiting;
            }
            AppEvent::QuestionsAnswered { request_id, .. } => {
                self.pending_attention
                    .remove(&format!("question:{request_id}"));
                self.clear_question_interaction(request_id);
                self.resume_active_task();
                self.status_tone = StatusTone::Active;
            }
            AppEvent::QuestionsFailed { request_id, reason } => {
                self.pending_attention
                    .remove(&format!("question:{request_id}"));
                self.clear_question_interaction(request_id);
                self.status = format!("Question closed · {}", sanitize_terminal_text(reason));
                self.resume_active_task();
                self.status_tone = StatusTone::Active;
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
                self.status_tone = StatusTone::Waiting;
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
                self.status_tone = StatusTone::Active;
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
                    crate::app::SecurityStatus::Unverified => "NOT VERIFIED",
                    crate::app::SecurityStatus::Verifying => "VERIFYING",
                    crate::app::SecurityStatus::UnattestedDevelopment
                        if crate::provider::unattested_development_enabled() =>
                    {
                        "UNATTESTED DEV"
                    }
                    crate::app::SecurityStatus::Verified => "SECURE",
                    crate::app::SecurityStatus::UnattestedDevelopment
                    | crate::app::SecurityStatus::Failed => "SECURITY FAILED",
                };
            }
            AppEvent::TurnCompleted { turn_id } => {
                self.running = false;
                self.status = "Task complete".into();
                self.status_tone = StatusTone::Success;
                self.clear_foreground_attention();
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
                self.status_tone = StatusTone::Cancelled;
                self.clear_foreground_attention();
                if let Some(index) = self.task_index(turn_id)
                    && let Some(task) = self.entries[index].task.as_mut()
                {
                    task.finish(TaskPhase::Cancelled, None);
                }
                self.transition_ticks = if self.options.animation { 2 } else { 0 };
            }
            AppEvent::ErrorRaised { turn_id, message } => {
                self.running = false;
                self.status_tone = StatusTone::Error;
                self.clear_foreground_attention();
                let detail = sanitize_terminal_text(message);
                self.status = if is_attestation_failure(&detail) {
                    "Attestation failed".into()
                } else {
                    "Error".into()
                };
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
            // `follow_output` is the viewport anchor. Keep `scroll` as a real
            // top-relative offset instead of overloading `u16::MAX` as a
            // sentinel; one upward navigation step can then leave follow mode.
            self.scroll = 0;
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
        if matches!(
            task.phase,
            TaskPhase::Starting | TaskPhase::Running | TaskPhase::Waiting
        ) {
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
                        let mut spans = self.task_rail_spans(task, theme, rail_row);
                        spans.append(&mut line.spans);
                        lines.push(Line::from(spans));
                        rail_row = rail_row.saturating_add(1);
                    }
                }
                TaskTimelineItem::Tool { call_id } => {
                    if let Some(tool) = task.tools.iter().find(|tool| tool.call_id == *call_id) {
                        let (glyph, color) = match tool.phase {
                            ToolPhase::Proposed | ToolPhase::Running => {
                                (self.blossom_glyph(false), theme.cherry_bright)
                            }
                            ToolPhase::Completed => (self.glyph("✓", "+"), theme.success),
                            ToolPhase::Failed => (self.glyph("✕", "x"), theme.error),
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

    /// A compacting-specific activity mark. Each cycle pulls three context
    /// blocks into one successor block, holds it briefly, then clears before
    /// beginning again. It intentionally lives beside the status text so the
    /// task outcome marker can remain static.
    fn compaction_frame(&self) -> &'static str {
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

    fn should_animate(&self) -> bool {
        self.options.animation
            && (self.running
                || self.security == "VERIFYING"
                || self.transition_ticks > 0
                || self.has_background_activity()
                || matches!(
                    self.overlay,
                    Some(Overlay::Auth(
                        AuthOverlay::Starting
                            | AuthOverlay::Browser { .. }
                            | AuthOverlay::Checking { .. }
                    ))
                ))
    }

    fn show_startup(&self) -> bool {
        !self.has_prompt
            && !self.running
            && self.entries.len() == 1
            && self.entries[0].kind == EntryKind::System
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

    fn reset_slash_completion(&mut self) {
        self.slash_completion = None;
        self.slash_selection = 0;
        self.slash_selection_moved = false;
    }

    fn begin_slash_input(&mut self) {
        self.focus = Focus::Composer;
        self.reset_slash_completion();
        self.input.insert_char('/');
    }

    fn invalid_slash_has_completion(&self) -> bool {
        self.input.starts_with('/')
            && slash::parse(&self.input).is_some_and(|command| command.is_err())
            && !slash::completions(&self.input).is_empty()
    }

    fn slash_candidates(&self) -> Vec<slash::SlashCompletion> {
        if let Some(cycle) = &self.slash_completion
            && cycle
                .candidates
                .get(cycle.index)
                .is_some_and(|candidate| candidate.input == self.input.as_str())
        {
            return cycle.candidates.clone();
        }
        slash::completions(&self.input)
    }

    fn can_navigate_slash(&self) -> bool {
        self.focus == Focus::Composer
            && !self.running
            && self.input.starts_with('/')
            && self.input.is_cursor_at_end()
            && !self.slash_candidates().is_empty()
    }

    fn move_slash_selection(&mut self, offset: isize) {
        let candidates = self.slash_candidates();
        if candidates.is_empty() {
            return;
        }
        self.slash_selection = if offset.is_negative() {
            self.slash_selection.saturating_sub(offset.unsigned_abs())
        } else {
            self.slash_selection
                .saturating_add(offset.unsigned_abs())
                .min(candidates.len() - 1)
        };
        self.slash_selection_moved = true;
        self.status = format!(
            "Option {}/{} · Enter select · Tab complete",
            self.slash_selection + 1,
            candidates.len()
        );
        self.status_tone = StatusTone::Neutral;
    }

    fn should_accept_slash_selection(&self) -> bool {
        let candidates = self.slash_candidates();
        let selected = candidates.get(self.slash_selection.min(candidates.len().saturating_sub(1)));
        selected.is_some_and(|candidate| {
            candidate.input != self.input.as_str()
                && (self.slash_selection_moved || self.invalid_slash_has_completion())
        })
    }

    fn clear_composer_draft(&mut self) -> bool {
        if self.input.is_empty() {
            return false;
        }
        self.input.clear();
        self.reset_slash_completion();
        self.focus = Focus::Composer;
        self.status = "Draft cleared".into();
        true
    }

    fn open_permissions(&mut self) {
        let selected = PermissionProfile::ALL
            .iter()
            .position(|profile| *profile == self.profile)
            .unwrap_or(0);
        self.overlay = Some(Overlay::Permissions {
            tab: PermissionTab::Tools,
            selected,
        });
        self.status = "Choose tool permissions".into();
        self.status_tone = StatusTone::Neutral;
    }

    fn switch_permission_tab(&mut self) {
        if let Some(Overlay::Permissions { tab, .. }) = &mut self.overlay {
            *tab = tab.other();
            self.status = match tab {
                PermissionTab::Tools => "Choose tool permissions".into(),
                PermissionTab::Crypto => "Crypto permissions are coming soon".into(),
            };
        }
    }

    fn move_permission_selection(&mut self, amount: isize) {
        let Some(Overlay::Permissions {
            tab: PermissionTab::Tools,
            selected,
        }) = &mut self.overlay
        else {
            return;
        };
        if amount.is_negative() {
            *selected = selected.saturating_sub(amount.unsigned_abs());
        } else {
            *selected = selected
                .saturating_add(amount.unsigned_abs())
                .min(PermissionProfile::ALL.len() - 1);
        }
    }

    fn selected_permission_profile(&self) -> Option<PermissionProfile> {
        let Overlay::Permissions {
            tab: PermissionTab::Tools,
            selected,
        } = self.overlay.as_ref()?
        else {
            return None;
        };
        PermissionProfile::ALL.get(*selected).copied()
    }

    fn open_model_picker(&mut self) {
        self.overlay = Some(Overlay::ModelPicker {
            query: String::new(),
            selected: 0,
        });
        self.align_model_picker_to_current();
        self.status = if self.model_catalog_loading {
            "Loading available models…".into()
        } else {
            "Select a model".into()
        };
    }

    fn model_picker_match_indices(&self) -> Vec<usize> {
        let Some(Overlay::ModelPicker { query, .. }) = &self.overlay else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        self.model_candidates
            .iter()
            .enumerate()
            .filter_map(|(index, model)| {
                (query.is_empty() || model.to_lowercase().contains(&query)).then_some(index)
            })
            .collect()
    }

    fn align_model_picker_to_current(&mut self) {
        let matches = self.model_picker_match_indices();
        let selected = matches
            .iter()
            .position(|index| self.model_candidates[*index] == self.model)
            .unwrap_or(0);
        if let Some(Overlay::ModelPicker {
            selected: picker_selected,
            ..
        }) = &mut self.overlay
        {
            *picker_selected = selected;
        }
    }

    fn move_model_picker(&mut self, amount: isize) {
        let match_count = self.model_picker_match_indices().len();
        let Some(Overlay::ModelPicker { selected, .. }) = &mut self.overlay else {
            return;
        };
        if match_count == 0 {
            *selected = 0;
        } else if amount.is_negative() {
            *selected = selected.saturating_sub(amount.unsigned_abs());
        } else {
            *selected = selected
                .saturating_add(amount.unsigned_abs())
                .min(match_count - 1);
        }
    }

    fn push_model_picker_search(&mut self, character: char) {
        if let Some(Overlay::ModelPicker { query, selected }) = &mut self.overlay {
            query.push(character);
            *selected = 0;
        }
    }

    fn pop_model_picker_search(&mut self) {
        if let Some(Overlay::ModelPicker { query, selected }) = &mut self.overlay {
            query.pop();
            *selected = 0;
        }
    }

    fn selected_model(&self) -> Option<String> {
        let Overlay::ModelPicker { selected, .. } = self.overlay.as_ref()? else {
            return None;
        };
        let model_index = *self.model_picker_match_indices().get(*selected)?;
        self.model_candidates.get(model_index).cloned()
    }

    fn open_resume_picker(&mut self, sessions: Vec<SessionSummary>) {
        self.status = if sessions.is_empty() {
            "No earlier transcripts in this workspace".into()
        } else {
            format!("Choose from {} saved transcript(s)", sessions.len())
        };
        self.overlay = Some(Overlay::ResumePicker {
            query: String::new(),
            selected: 0,
            sessions,
        });
    }

    fn resume_picker_match_indices(&self) -> Vec<usize> {
        let Some(Overlay::ResumePicker {
            query, sessions, ..
        }) = &self.overlay
        else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| {
                let title = session.title.as_deref().unwrap_or("Untitled transcript");
                (query.is_empty()
                    || title.to_lowercase().contains(&query)
                    || session.id.to_lowercase().contains(&query)
                    || session.updated_at.to_lowercase().contains(&query))
                .then_some(index)
            })
            .collect()
    }

    fn move_resume_picker(&mut self, amount: isize) {
        let match_count = self.resume_picker_match_indices().len();
        let Some(Overlay::ResumePicker { selected, .. }) = &mut self.overlay else {
            return;
        };
        if match_count == 0 {
            *selected = 0;
        } else if amount.is_negative() {
            *selected = selected.saturating_sub(amount.unsigned_abs());
        } else {
            *selected = selected
                .saturating_add(amount.unsigned_abs())
                .min(match_count - 1);
        }
    }

    fn push_resume_picker_search(&mut self, character: char) {
        if let Some(Overlay::ResumePicker {
            query, selected, ..
        }) = &mut self.overlay
        {
            query.push(character);
            *selected = 0;
        }
    }

    fn pop_resume_picker_search(&mut self) {
        if let Some(Overlay::ResumePicker {
            query, selected, ..
        }) = &mut self.overlay
        {
            query.pop();
            *selected = 0;
        }
    }

    fn selected_resume_session(&self) -> Option<SessionSummary> {
        let Overlay::ResumePicker {
            selected, sessions, ..
        } = self.overlay.as_ref()?
        else {
            return None;
        };
        let session_index = *self.resume_picker_match_indices().get(*selected)?;
        sessions.get(session_index).cloned()
    }

    fn open_delete_picker(&mut self, sessions: Vec<SessionSummary>) {
        self.status = if sessions.is_empty() {
            "No saved transcripts available to delete".into()
        } else {
            format!("Choose transcripts to delete from {} saved", sessions.len())
        };
        self.overlay = Some(Overlay::DeletePicker {
            query: String::new(),
            selected: 0,
            sessions,
            marked: BTreeSet::new(),
            confirming: false,
        });
    }

    fn delete_picker_match_indices(&self) -> Vec<usize> {
        let Some(Overlay::DeletePicker {
            query, sessions, ..
        }) = &self.overlay
        else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        sessions
            .iter()
            .enumerate()
            .filter_map(|(index, session)| {
                let title = session.title.as_deref().unwrap_or("Untitled transcript");
                let cwd = session.cwd.to_string_lossy();
                (query.is_empty()
                    || title.to_lowercase().contains(&query)
                    || session.id.to_lowercase().contains(&query)
                    || session.updated_at.to_lowercase().contains(&query)
                    || cwd.to_lowercase().contains(&query))
                .then_some(index)
            })
            .collect()
    }

    fn move_delete_picker(&mut self, amount: isize) {
        let match_count = self.delete_picker_match_indices().len();
        let Some(Overlay::DeletePicker { selected, .. }) = &mut self.overlay else {
            return;
        };
        if match_count == 0 {
            *selected = 0;
        } else if amount.is_negative() {
            *selected = selected.saturating_sub(amount.unsigned_abs());
        } else {
            *selected = selected
                .saturating_add(amount.unsigned_abs())
                .min(match_count - 1);
        }
    }

    fn push_delete_picker_search(&mut self, character: char) {
        if let Some(Overlay::DeletePicker {
            query,
            selected,
            confirming,
            ..
        }) = &mut self.overlay
            && !*confirming
        {
            query.push(character);
            *selected = 0;
        }
    }

    fn pop_delete_picker_search(&mut self) {
        if let Some(Overlay::DeletePicker {
            query,
            selected,
            confirming,
            ..
        }) = &mut self.overlay
            && !*confirming
        {
            query.pop();
            *selected = 0;
        }
    }

    fn toggle_delete_picker_session(&mut self) {
        let Some(session_id) = (|| {
            let Overlay::DeletePicker {
                selected, sessions, ..
            } = self.overlay.as_ref()?
            else {
                return None;
            };
            let index = *self.delete_picker_match_indices().get(*selected)?;
            Some(sessions.get(index)?.id.clone())
        })() else {
            return;
        };
        if let Some(Overlay::DeletePicker { marked, .. }) = &mut self.overlay
            && !marked.remove(&session_id)
        {
            marked.insert(session_id);
        }
    }

    fn toggle_all_delete_picker_matches(&mut self) {
        let ids = {
            let Some(Overlay::DeletePicker { sessions, .. }) = &self.overlay else {
                return;
            };
            self.delete_picker_match_indices()
                .into_iter()
                .filter_map(|index| sessions.get(index).map(|session| session.id.clone()))
                .collect::<Vec<_>>()
        };
        let Some(Overlay::DeletePicker { marked, .. }) = &mut self.overlay else {
            return;
        };
        if !ids.is_empty() && ids.iter().all(|id| marked.contains(id)) {
            for id in ids {
                marked.remove(&id);
            }
        } else {
            marked.extend(ids);
        }
    }

    fn begin_delete_confirmation(&mut self) {
        let Some(Overlay::DeletePicker {
            marked, confirming, ..
        }) = &mut self.overlay
        else {
            return;
        };
        if marked.is_empty() {
            self.status = "Select at least one transcript to delete".into();
        } else {
            *confirming = true;
            self.status = format!(
                "Confirm permanent deletion of {} transcript(s)",
                marked.len()
            );
            self.status_tone = StatusTone::Waiting;
        }
    }

    fn marked_delete_session_ids(&self) -> Result<Vec<SessionId>> {
        let Some(Overlay::DeletePicker { marked, .. }) = &self.overlay else {
            return Ok(Vec::new());
        };
        marked
            .iter()
            .map(|id| {
                SessionId::from_str(id).map_err(|_| {
                    AxiomError::Storage("saved transcript has an invalid session ID".into())
                })
            })
            .collect()
    }

    fn complete_slash(&mut self) {
        if self.slash_selection_moved {
            self.accept_slash_selection();
            return;
        }
        if let Some(cycle) = self.slash_completion.as_mut()
            && cycle
                .candidates
                .get(cycle.index)
                .is_some_and(|candidate| candidate.input == self.input.as_str())
        {
            cycle.index = (cycle.index + 1) % cycle.candidates.len();
            self.input
                .set_text(cycle.candidates[cycle.index].input.clone());
            self.slash_selection = cycle.index;
            self.status = format!(
                "Completion {}/{} · Tab next · Enter run",
                cycle.index + 1,
                cycle.candidates.len()
            );
            return;
        }

        let candidates = slash::completions(&self.input);
        if candidates.is_empty() {
            self.reset_slash_completion();
            self.status = "No completion available".into();
            return;
        }
        let index = self.slash_selection.min(candidates.len() - 1);
        self.input.set_text(candidates[index].input.clone());
        self.slash_selection = index;
        self.slash_selection_moved = false;
        self.status = format!(
            "Completion {}/{} · Tab next · Enter run",
            index + 1,
            candidates.len()
        );
        self.slash_completion = Some(SlashCompletionCycle { candidates, index });
    }

    fn accept_slash_selection(&mut self) {
        let candidates = self.slash_candidates();
        if candidates.is_empty() {
            self.reset_slash_completion();
            self.status = "No completion available".into();
            return;
        }
        let index = self.slash_selection.min(candidates.len() - 1);
        self.input.set_text(candidates[index].input.clone());
        self.slash_selection = index;
        self.slash_selection_moved = false;
        self.status = format!(
            "Completion {}/{} · Tab next · Enter run",
            index + 1,
            candidates.len()
        );
        self.slash_completion = Some(SlashCompletionCycle { candidates, index });
    }
}

fn stop_running_turn(state: &mut TuiState, cancellation: Option<&CancellationToken>) -> bool {
    if !state.running {
        return false;
    }
    let Some(cancellation) = cancellation else {
        return false;
    };
    cancellation.cancel();
    state.exit_armed = false;
    state.overlay = None;
    state.overlay_scroll = 0;
    state.search_active = false;
    state.search_query.clear();
    state.answer_input.clear();
    state.answer_prompt = None;
    state.interaction = None;
    state.focus = Focus::Composer;
    state.status = if state.compacting {
        "Stopping compaction…".into()
    } else {
        "Stopping task…".into()
    };
    state.status_tone = StatusTone::Cancelled;
    if let Some(task) = state.active_task_mut() {
        task.activity = TaskActivity::Working("Stopping the task".into());
    }
    true
}

fn slash_help_text() -> String {
    let mut text = String::from("SLASH COMMANDS\n");
    for spec in slash::COMMANDS {
        let _ = writeln!(text, "{}\n  {}", spec.usage, spec.description);
    }
    text.push_str("\nCommands run locally and are never sent to the model as prompts.");
    text
}

fn show_slash_error(state: &mut TuiState, message: &str) {
    let message = sanitize_terminal_text(message);
    state.status = format!("Command error: {message}");
    state.status_tone = StatusTone::Error;
}

fn account_status_text(account: &AccountStatus) -> String {
    match account.source {
        crate::auth::CredentialSource::Environment => {
            "Using the Axiom API key from AXIOM_API_KEY.".into()
        }
        crate::auth::CredentialSource::SystemKeyring => {
            "Your Axiom API key is saved securely in the system keyring.".into()
        }
        crate::auth::CredentialSource::ProtectedFile => {
            "Your Axiom API key is saved in a protected local file.".into()
        }
    }
}

enum UiMessage {
    Event(AppEvent),
    Done(TurnId, Result<()>),
    SecurityPreflightDone(u64, Result<SecurityVerification>),
    CompactionDone(Result<CompactionResult>),
    ModelsLoaded(Result<Vec<String>>),
    TitleGenerated(SessionId, Result<String>),
    Approval(ApprovalRequest, oneshot::Sender<ApprovalResponse>),
    Questions(
        QuestionRequest,
        oneshot::Sender<Result<BTreeMap<String, Vec<String>>>>,
    ),
    BrowserLoginStarted(Box<Result<BrowserLogin>>),
    AuthCompleted(Result<AccountStatus>),
    AuthValidated(ValidationPurpose, ValidationStatus),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValidationPurpose {
    Startup,
    Login,
    Account,
}

fn start_security_preflight(
    state: &mut TuiState,
    runner: &Arc<dyn TurnRunner>,
    ui_tx: &mpsc::Sender<UiMessage>,
    cancellation: &mut Option<CancellationToken>,
    generation: &mut u64,
) {
    if let Some(previous) = cancellation.take() {
        previous.cancel();
    }
    let token = CancellationToken::new();
    *cancellation = Some(token.clone());
    *generation = generation.wrapping_add(1);
    let request_generation = *generation;
    state.apply(&AppEvent::SecurityStatusChanged {
        state: crate::app::SecurityStatus::Verifying,
    });
    state.security_evidence = None;
    let task_runner = Arc::clone(runner);
    let model = state.model.clone();
    let result_tx = ui_tx.clone();
    tokio::spawn(async move {
        let result = task_runner.verify_security(&model, token.clone()).await;
        let result = if token.is_cancelled() {
            Err(AxiomError::Cancelled)
        } else {
            result
        };
        let _ = result_tx
            .send(UiMessage::SecurityPreflightDone(request_generation, result))
            .await;
    });
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
    drafts: BTreeMap<String, String>,
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
        "Question {}/{}{}: {}{}",
        pending.index + 1,
        pending.request.questions.len(),
        if question.required {
            ""
        } else {
            " · optional"
        },
        question.prompt,
        if options.is_empty() {
            String::new()
        } else {
            format!(" · {options}")
        }
    )
}

fn parse_tui_answer(input: &str, question: &crate::app::QuestionSpec) -> Result<Vec<String>> {
    if input.trim().is_empty() && !question.required {
        return Ok(Vec::new());
    }
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

fn restore_question_draft(pending: &PendingQuestions) -> String {
    let question = &pending.request.questions[pending.index];
    pending
        .drafts
        .get(&question.id)
        .cloned()
        .or_else(|| {
            pending.answers.get(&question.id).map(|answers| {
                if question.options.is_empty() {
                    answers.join(", ")
                } else {
                    answers
                        .iter()
                        .filter_map(|answer| {
                            question
                                .options
                                .iter()
                                .position(|option| option == answer)
                                .map(|index| (index + 1).to_string())
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                }
            })
        })
        .unwrap_or_default()
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

struct TerminalGuard {
    reset_background: bool,
}

impl TerminalGuard {
    fn enter(background: Option<&'static str>) -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(
            io::stdout(),
            SetAxiomTerminalBackground(background),
            EnterAlternateScreen,
            EnableBracketedPaste
        )?;
        Ok(Self {
            reset_background: background.is_some(),
        })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            ResetAxiomTerminalBackground(self.reset_background)
        );
    }
}

async fn restore_tui_session(
    runtime: &Runtime,
    runner: &Arc<dyn TurnRunner>,
    store: &SessionStore,
    cwd: &PathBuf,
    session_id: &SessionId,
    mut state: TuiState,
) -> Result<TuiState> {
    let summary = store.summary(session_id)?;
    if summary.cwd.canonicalize()? != *cwd {
        return Err(AxiomError::Storage(format!(
            "resume cwd does not match journaled workspace {}",
            summary.cwd.display()
        )));
    }
    let loaded = store.load_recovering(session_id)?;
    let resumed_model = model_for_resume(&loaded.events, &state.model);
    let resumed_thinking = thinking_for_resume(&loaded.events, state.thinking);
    let resumed_profile = permission_for_resume(&loaded.events, state.profile);
    runtime.restore(&loaded.events).await?;
    runner.restore_session(session_id, &loaded.events).await?;
    state.profile = PermissionProfile::from_str(&summary.profile).unwrap_or(state.profile);
    for envelope in &loaded.events {
        state.apply(&envelope.event);
    }
    state.model = resumed_model;
    state.thinking = resumed_thinking;
    state.profile = resumed_profile;
    // Security is live process state, not resumable conversation state. A
    // historical receipt must never become the current header claim.
    state.reset_security();
    state.running = false;
    state.clear_foreground_attention();
    state.status = "Resumed · no interrupted actions replayed".into();
    let resumed = runtime
        .emit(
            session_id.clone(),
            AppEvent::SessionResumed {
                cwd: cwd.clone(),
                origin: Origin::Tui,
                profile: state.profile,
            },
        )
        .await?;
    store.append(&resumed)?;
    state.apply(&resumed.event);
    let recovery = store.recovery_status(session_id)?;
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
    Ok(state)
}

pub async fn run(
    runner: Arc<dyn TurnRunner>,
    cwd: PathBuf,
    model: String,
    profile: PermissionProfile,
    launch: TuiLaunch,
) -> Result<()> {
    let TuiLaunch {
        options,
        store,
        resume,
        auth,
    } = launch;
    let cwd = cwd.canonicalize()?;
    let default_model = model.clone();
    let default_thinking = ThinkingLevel::Medium;
    let initial_model = if resume.is_none() {
        store
            .as_ref()
            .map(SessionStore::last_used_model)
            .transpose()?
            .flatten()
            .unwrap_or(model)
    } else {
        model
    };
    let initial_thinking = if resume.is_none() {
        store
            .as_ref()
            .map(SessionStore::last_used_thinking)
            .transpose()?
            .flatten()
            .unwrap_or(default_thinking)
    } else {
        default_thinking
    };
    let _guard = TerminalGuard::enter(options.color_mode.terminal_background())?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let runtime = Runtime::new(512);
    let mut state = TuiState::with_options(cwd.clone(), initial_model, profile, options);
    state.thinking = initial_thinking;
    let mut session_id = if let Some(session_id) = resume {
        let store = store
            .as_ref()
            .ok_or_else(|| AxiomError::Storage("session resume requires a durable store".into()))?;
        state = restore_tui_session(
            &runtime,
            &runner,
            store,
            &cwd,
            &session_id,
            TuiState::with_options(cwd.clone(), default_model.clone(), profile, options),
        )
        .await?;
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
        if state.model != default_model {
            runner.set_model(&session_id, state.model.clone()).await?;
        }
        if state.thinking != default_thinking {
            runner
                .set_thinking_level(&session_id, state.thinking)
                .await?;
        }
        let selected_model = runtime
            .dispatch(AppCommand::ChangeModel {
                session_id: session_id.clone(),
                model: state.model.clone(),
            })
            .await?;
        if let Some(store) = &store {
            store.append_all(&selected_model)?;
        }
        for event in selected_model {
            state.apply(&event.event);
        }
        let selected_thinking = runtime
            .dispatch(AppCommand::ChangeThinkingLevel {
                session_id: session_id.clone(),
                level: state.thinking,
            })
            .await?;
        if let Some(store) = &store {
            store.append_all(&selected_thinking)?;
        }
        for event in selected_thinking {
            state.apply(&event.event);
        }
        state.status = "Ready".into();
        state.status_tone = StatusTone::Success;
        session_id
    };

    let mut input_events = EventStream::new();
    let (ui_tx, mut ui_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let mut cancellation: Option<CancellationToken> = None;
    let mut auth_cancellation: Option<CancellationToken> = None;
    let mut security_cancellation: Option<CancellationToken> = None;
    let mut security_generation = 0_u64;
    let mut startup_auth_pending = false;
    let mut active_turn: Option<TurnId> = None;
    let mut pending_approval: Option<(ApprovalRequest, oneshot::Sender<ApprovalResponse>)> = None;
    let mut pending_questions: Option<PendingQuestions> = None;
    let mut animation_ticks = tokio::time::interval(ANIMATION_TICK);
    animation_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    if uses_deterministic_test_runner() {
        if state.status == "Ready" {
            state.status = "Ready · deterministic development runner".into();
        }
    } else if auth.has_credential() {
        startup_auth_pending = true;
        state.status = "Checking Axiom account…".into();
        let validation_auth = auth.clone();
        let validation_tx = ui_tx.clone();
        tokio::spawn(async move {
            let status = validation_auth.validate().await;
            let _ = validation_tx
                .send(UiMessage::AuthValidated(ValidationPurpose::Startup, status))
                .await;
        });
    } else {
        state.open_login(None);
    }

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
                        if key.code == KeyCode::Esc
                            && stop_running_turn(&mut state, cancellation.as_ref())
                        {
                            // The runner reports `Done(Cancelled)` after its provider,
                            // tool, approval, or question wait has terminated.
                        } else if key.modifiers.contains(KeyModifiers::CONTROL)
                            && key.code == KeyCode::Char('c')
                        {
                            if state.exit_armed {
                                if let Some(token) = &cancellation {
                                    token.cancel();
                                }
                                if let Some(token) = security_cancellation.take() {
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
                                            pending
                                                .drafts
                                                .insert(question.id.clone(), state.answer_input.clone());
                                            if values.is_empty() {
                                                pending.answers.remove(&question.id);
                                            } else {
                                                pending.answers.insert(question.id.clone(), values);
                                            }
                                            pending.index += 1;
                                            state.answer_input.clear();
                                            if pending.index == pending.request.questions.len() {
                                                let completed = pending_questions.take().expect("pending questions exist");
                                                let _ = completed.response.send(Ok(completed.answers));
                                                state.answer_prompt = None;
                                                state.interaction = None;
                                                state.status = "Answers submitted".into();
                                            } else {
                                                state.answer_input = restore_question_draft(pending);
                                                state.answer_prompt = Some(question_status(pending));
                                                state.interaction = Some(InteractionView::Questions {
                                                    request: pending.request.clone(),
                                                    index: pending.index,
                                                });
                                                state.status = state.answer_prompt.clone().unwrap_or_default();
                                            }
                                        }
                                        Err(error) => state.status = error.to_string(),
                                    }
                                }
                                KeyCode::BackTab => {
                                    let pending = pending_questions.as_mut().expect("pending questions exist");
                                    let question = &pending.request.questions[pending.index];
                                    pending
                                        .drafts
                                        .insert(question.id.clone(), state.answer_input.clone());
                                    if pending.index > 0 {
                                        pending.index -= 1;
                                        state.answer_input = restore_question_draft(pending);
                                        state.answer_prompt = Some(question_status(pending));
                                        state.interaction = Some(InteractionView::Questions {
                                            request: pending.request.clone(),
                                            index: pending.index,
                                        });
                                        state.status = state.answer_prompt.clone().unwrap_or_default();
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
                                    state.interaction = None;
                                    state.status = format!("Allowed once · {}", request.request_id);
                                }
                                KeyCode::Char('e' | 'E') if request.allow_session_grants => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse { choice: ApprovalChoice::AllowExactSession });
                                    state.interaction = None;
                                    state.status = format!("Allowed exact operation for session · {}", request.request_id);
                                }
                                KeyCode::Char('p' | 'P') if request.suggested_prefix_scope.is_some() => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse { choice: ApprovalChoice::AllowPrefixSession });
                                    state.interaction = None;
                                    state.status = format!("Allowed safe scope for session · {}", request.request_id);
                                }
                                KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                                    let (request, response) = pending_approval.take().expect("pending approval exists");
                                    let _ = response.send(ApprovalResponse::deny());
                                    state.interaction = None;
                                    state.status = format!("Denied {}", request.request_id);
                                }
                                _ => {
                                    state.status = approval_status(request);
                                }
                            }
                        } else if matches!(state.overlay, Some(Overlay::Auth(_))) {
                            state.exit_armed = false;
                            match &state.overlay {
                                Some(Overlay::Auth(AuthOverlay::Menu { .. })) => match key.code {
                                    KeyCode::Esc => state.overlay = None,
                                    KeyCode::Up => state.auth_menu_move(-1),
                                    KeyCode::Down => state.auth_menu_move(1),
                                    KeyCode::Enter => {
                                        let selected = match &state.overlay {
                                            Some(Overlay::Auth(AuthOverlay::Menu { selected, .. })) => *selected,
                                            _ => 0,
                                        };
                                        match selected {
                                            0 => {
                                                state.overlay = Some(Overlay::Auth(AuthOverlay::Starting));
                                                state.status = "Opening secure browser authorization…".into();
                                                let token = CancellationToken::new();
                                                auth_cancellation = Some(token.clone());
                                                let login_auth = auth.clone();
                                                let login_tx = ui_tx.clone();
                                                tokio::spawn(async move {
                                                    let result = tokio::select! {
                                                        () = token.cancelled() => Err(AxiomError::Cancelled),
                                                        result = login_auth.start_browser_login() => result,
                                                    };
                                                    let _ = login_tx
                                                        .send(UiMessage::BrowserLoginStarted(Box::new(result)))
                                                        .await;
                                                });
                                            }
                                            1 => {
                                                state.overlay = Some(Overlay::Auth(AuthOverlay::Paste {
                                                    kind: AuthPasteKind::ApiKey,
                                                    input: SecretInput(String::new()),
                                                }));
                                                state.status = "Paste your Axiom API key".into();
                                            }
                                            _ => unreachable!("the sign-in menu has two options"),
                                        }
                                    }
                                    _ => {}
                                },
                                Some(Overlay::Auth(AuthOverlay::Starting)) => {
                                    if key.code == KeyCode::Esc {
                                        if let Some(token) = auth_cancellation.take() {
                                            token.cancel();
                                        }
                                        state.open_login(Some("Browser sign-in cancelled.".into()));
                                    }
                                }
                                Some(Overlay::Auth(AuthOverlay::Browser { .. })) => match key.code {
                                    KeyCode::Esc => {
                                        if let Some(token) = auth_cancellation.take() {
                                            token.cancel();
                                        }
                                        state.open_login(Some("Browser sign-in cancelled.".into()));
                                    }
                                    KeyCode::Char('p' | 'P') => {
                                        if let Some(token) = auth_cancellation.take() {
                                            token.cancel();
                                        }
                                        state.overlay = Some(Overlay::Auth(AuthOverlay::Paste {
                                            kind: AuthPasteKind::CompletionCode,
                                            input: SecretInput(String::new()),
                                        }));
                                        state.status = "Paste the one-time browser code".into();
                                    }
                                    KeyCode::Char('o' | 'O') => {
                                        if let Some(Overlay::Auth(AuthOverlay::Browser {
                                            authorization_url,
                                            ..
                                        })) = &state.overlay
                                        {
                                            let _ = open::that_detached(authorization_url);
                                        }
                                    }
                                    _ => {}
                                },
                                Some(Overlay::Auth(AuthOverlay::Paste { .. })) => match key.code {
                                    KeyCode::Esc => state.open_login(None),
                                    KeyCode::Backspace => state.auth_secret_pop(),
                                    KeyCode::Enter => {
                                        let (kind, value) = match &mut state.overlay {
                                            Some(Overlay::Auth(AuthOverlay::Paste { kind, input })) => {
                                                (*kind, std::mem::take(&mut input.0))
                                            }
                                            _ => unreachable!("auth paste overlay is active"),
                                        };
                                        if value.trim().is_empty() {
                                            state.open_login(Some("No authorization value was entered.".into()));
                                        } else if kind == AuthPasteKind::ApiKey {
                                            match auth.save_pasted_key(&value) {
                                                Ok(()) => {
                                                    state.reset_security();
                                                    state.overlay = Some(Overlay::Auth(AuthOverlay::Checking {
                                                        label: "Checking Axiom API key…",
                                                    }));
                                                    let validation_auth = auth.clone();
                                                    let validation_tx = ui_tx.clone();
                                                    tokio::spawn(async move {
                                                        let status = validation_auth.validate().await;
                                                        let _ = validation_tx
                                                            .send(UiMessage::AuthValidated(
                                                                ValidationPurpose::Login,
                                                                status,
                                                            ))
                                                            .await;
                                                    });
                                                }
                                                Err(error) => state.open_login(Some(error.to_string())),
                                            }
                                        } else {
                                            state.overlay = Some(Overlay::Auth(AuthOverlay::Checking {
                                                label: "Exchanging one-time browser code…",
                                            }));
                                            let completion_auth = auth.clone();
                                            let completion_tx = ui_tx.clone();
                                            tokio::spawn(async move {
                                                let result = completion_auth.complete_with_code(&value).await;
                                                let _ = completion_tx
                                                    .send(UiMessage::AuthCompleted(result))
                                                    .await;
                                            });
                                        }
                                    }
                                    KeyCode::Char(character)
                                        if key.modifiers.is_empty()
                                            || key.modifiers == KeyModifiers::SHIFT =>
                                    {
                                        state.auth_secret_push(&character.to_string());
                                    }
                                    _ => {}
                                },
                                Some(Overlay::Auth(AuthOverlay::Account { .. })) => {
                                    if matches!(key.code, KeyCode::Esc | KeyCode::Enter) {
                                        state.overlay = None;
                                    }
                                }
                                _ => {}
                            }
                        } else if matches!(state.overlay, Some(Overlay::Permissions { .. })) {
                            state.exit_armed = false;
                            match key.code {
                                KeyCode::Esc => {
                                    state.overlay = None;
                                    state.status = "Permission selection cancelled".into();
                                }
                                KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                                    state.switch_permission_tab();
                                }
                                KeyCode::Up => state.move_permission_selection(-1),
                                KeyCode::Down => state.move_permission_selection(1),
                                KeyCode::PageUp => state.move_permission_selection(-4),
                                KeyCode::PageDown => state.move_permission_selection(4),
                                KeyCode::Enter => {
                                    if let Some(next_profile) =
                                        state.selected_permission_profile()
                                    {
                                        state.overlay = None;
                                        match runtime
                                            .dispatch(AppCommand::ChangePermissionProfile {
                                                session_id: session_id.clone(),
                                                profile: next_profile,
                                            })
                                            .await
                                        {
                                            Ok(events) => {
                                                if let Some(store) = &store {
                                                    store.append_all(&events)?;
                                                }
                                                for event in events {
                                                    state.apply(&event.event);
                                                }
                                            }
                                            Err(error) => show_slash_error(
                                                &mut state,
                                                &error.to_string(),
                                            ),
                                        }
                                    } else {
                                        state.status =
                                            "Crypto permissions are coming soon".into();
                                        state.status_tone = StatusTone::Neutral;
                                    }
                                }
                                _ => {}
                            }
                        } else if matches!(state.overlay, Some(Overlay::ModelPicker { .. })) {
                            state.exit_armed = false;
                            match key.code {
                                KeyCode::Esc => {
                                    state.overlay = None;
                                    state.status = "Model selection cancelled".into();
                                }
                                KeyCode::Up => state.move_model_picker(-1),
                                KeyCode::Down => state.move_model_picker(1),
                                KeyCode::PageUp => state.move_model_picker(-8),
                                KeyCode::PageDown => state.move_model_picker(8),
                                KeyCode::Backspace => state.pop_model_picker_search(),
                                KeyCode::Char(character)
                                    if key.modifiers.is_empty()
                                        || key.modifiers == KeyModifiers::SHIFT =>
                                {
                                    state.push_model_picker_search(character);
                                }
                                KeyCode::Enter => {
                                    if let Some(model) = state.selected_model() {
                                        state.overlay = None;
                                        state.status = format!("Switching to {model}…");
                                        match runner.set_model(&session_id, model.clone()).await {
                                            Ok(()) => match runtime
                                                .dispatch(AppCommand::ChangeModel {
                                                    session_id: session_id.clone(),
                                                    model,
                                                })
                                                .await
                                            {
                                                Ok(events) => {
                                                    if let Some(store) = &store {
                                                        store.append_all(&events)?;
                                                    }
                                                    for event in events {
                                                        state.apply(&event.event);
                                                    }
                                                    if auth.has_credential() {
                                                        start_security_preflight(
                                                            &mut state,
                                                            &runner,
                                                            &ui_tx,
                                                            &mut security_cancellation,
                                                            &mut security_generation,
                                                        );
                                                    }
                                                }
                                                Err(error) => show_slash_error(
                                                    &mut state,
                                                    &error.to_string(),
                                                ),
                                            },
                                            Err(error) => {
                                                show_slash_error(&mut state, &error.to_string());
                                            }
                                        }
                                    } else if !state.model_catalog_loading {
                                        state.status = "No matching model to select".into();
                                    }
                                }
                                _ => {}
                            }
                        } else if matches!(state.overlay, Some(Overlay::ResumePicker { .. })) {
                            state.exit_armed = false;
                            match key.code {
                                KeyCode::Esc => {
                                    state.overlay = None;
                                    state.status = "Resume cancelled".into();
                                }
                                KeyCode::Up => state.move_resume_picker(-1),
                                KeyCode::Down => state.move_resume_picker(1),
                                KeyCode::PageUp => state.move_resume_picker(-8),
                                KeyCode::PageDown => state.move_resume_picker(8),
                                KeyCode::Backspace => state.pop_resume_picker_search(),
                                KeyCode::Char(character)
                                    if key.modifiers.is_empty()
                                        || key.modifiers == KeyModifiers::SHIFT =>
                                {
                                    state.push_resume_picker_search(character);
                                }
                                KeyCode::Enter => {
                                    let selected = state.selected_resume_session();
                                    state.overlay = None;
                                    let Some(summary) = selected else {
                                        state.status = "No matching transcript to resume".into();
                                        continue;
                                    };
                                    let Some(store) = store.as_ref() else {
                                        show_slash_error(
                                            &mut state,
                                            "session resume requires a durable store",
                                        );
                                        continue;
                                    };
                                    let Ok(target) = SessionId::from_str(&summary.id) else {
                                        show_slash_error(
                                            &mut state,
                                            "saved transcript has an invalid session ID",
                                        );
                                        continue;
                                    };
                                    state.status = "Restoring transcript…".into();
                                    match restore_tui_session(
                                        &runtime,
                                        &runner,
                                        store,
                                        &cwd,
                                        &target,
                                        TuiState::with_options(
                                            cwd.clone(),
                                            default_model.clone(),
                                            profile,
                                            options,
                                        ),
                                    )
                                    .await
                                    {
                                        Ok(restored) => {
                                            state = restored;
                                            session_id = target;
                                            cancellation = None;
                                            active_turn = None;
                                            pending_approval = None;
                                            pending_questions = None;
                                            if auth.has_credential() {
                                                start_security_preflight(
                                                    &mut state,
                                                    &runner,
                                                    &ui_tx,
                                                    &mut security_cancellation,
                                                    &mut security_generation,
                                                );
                                            }
                                        }
                                        Err(error) => {
                                            show_slash_error(&mut state, &error.to_string());
                                        }
                                    }
                                }
                                _ => {}
                            }
                        } else if matches!(state.overlay, Some(Overlay::DeletePicker { .. })) {
                            state.exit_armed = false;
                            let confirming = matches!(
                                state.overlay,
                                Some(Overlay::DeletePicker {
                                    confirming: true,
                                    ..
                                })
                            );
                            if confirming {
                                match key.code {
                                    KeyCode::Char('y' | 'Y') => {
                                        let ids = state.marked_delete_session_ids();
                                        match (store.as_ref(), ids) {
                                            (Some(store), Ok(ids)) => {
                                                match store.delete_sessions(&ids) {
                                                    Ok(deleted) => {
                                                        state.overlay = None;
                                                        state.status = format!(
                                                            "Deleted {deleted} transcript(s) permanently"
                                                        );
                                                        state.status_tone = StatusTone::Success;
                                                    }
                                                    Err(error) => show_slash_error(
                                                        &mut state,
                                                        &error.to_string(),
                                                    ),
                                                }
                                            }
                                            (None, _) => show_slash_error(
                                                &mut state,
                                                "session deletion requires a durable store",
                                            ),
                                            (_, Err(error)) => show_slash_error(
                                                &mut state,
                                                &error.to_string(),
                                            ),
                                        }
                                    }
                                    KeyCode::Char('n' | 'N') | KeyCode::Esc => {
                                        if let Some(Overlay::DeletePicker {
                                            confirming, ..
                                        }) = &mut state.overlay
                                        {
                                            *confirming = false;
                                        }
                                        state.status = "Deletion cancelled · selection kept".into();
                                        state.status_tone = StatusTone::Neutral;
                                    }
                                    _ => {}
                                }
                            } else {
                                match key.code {
                                    KeyCode::Esc => {
                                        state.overlay = None;
                                        state.status = "Deletion cancelled".into();
                                    }
                                    KeyCode::Up => state.move_delete_picker(-1),
                                    KeyCode::Down => state.move_delete_picker(1),
                                    KeyCode::PageUp => state.move_delete_picker(-8),
                                    KeyCode::PageDown => state.move_delete_picker(8),
                                    KeyCode::Backspace => state.pop_delete_picker_search(),
                                    KeyCode::Char('a')
                                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                                    {
                                        state.toggle_all_delete_picker_matches();
                                    }
                                    KeyCode::Char(' ') => state.toggle_delete_picker_session(),
                                    KeyCode::Char(character)
                                        if key.modifiers.is_empty()
                                            || key.modifiers == KeyModifiers::SHIFT =>
                                    {
                                        state.push_delete_picker_search(character);
                                    }
                                    KeyCode::Enter => state.begin_delete_confirmation(),
                                    _ => {}
                                }
                            }
                        } else if matches!(state.overlay, Some(Overlay::SaveAttestation(_))) {
                            state.exit_armed = false;
                            let focus = match &state.overlay {
                                Some(Overlay::SaveAttestation(picker)) => picker.focus,
                                _ => unreachable!("attestation save picker is active"),
                            };
                            match key.code {
                                KeyCode::Esc => state.close_attestation_save_picker(),
                                KeyCode::Tab => state.toggle_attestation_save_focus(),
                                KeyCode::Up => state.move_attestation_directory_selection(-1),
                                KeyCode::Down => state.move_attestation_directory_selection(1),
                                KeyCode::PageUp => {
                                    state.move_attestation_directory_selection(-8);
                                }
                                KeyCode::PageDown => {
                                    state.move_attestation_directory_selection(8);
                                }
                                KeyCode::Left | KeyCode::Backspace
                                    if focus == SavePickerFocus::Directories =>
                                {
                                    state.open_parent_attestation_directory();
                                }
                                KeyCode::Enter if focus == SavePickerFocus::Directories => {
                                    state.open_selected_attestation_directory();
                                }
                                KeyCode::Enter => state.save_attestation_export(),
                                KeyCode::Char('s' | 'S')
                                    if focus == SavePickerFocus::Directories =>
                                {
                                    state.save_attestation_export();
                                }
                                KeyCode::Char('u')
                                    if focus == SavePickerFocus::Filename
                                        && key.modifiers.contains(KeyModifiers::CONTROL) =>
                                {
                                    state.clear_attestation_filename();
                                }
                                KeyCode::Backspace => state.pop_attestation_filename(),
                                KeyCode::Char(character)
                                    if focus == SavePickerFocus::Filename
                                        && (key.modifiers.is_empty()
                                            || key.modifiers == KeyModifiers::SHIFT) =>
                                {
                                    state.push_attestation_filename(&character.to_string());
                                }
                                _ => {}
                            }
                        } else if matches!(state.overlay, Some(Overlay::Security(_))) {
                            state.exit_armed = false;
                            match key.code {
                                KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                                    state.overlay = None;
                                    state.overlay_scroll = 0;
                                }
                                KeyCode::Char('v' | 'V') | KeyCode::Enter
                                | KeyCode::Left | KeyCode::Right => {
                                    state.cycle_security_view();
                                }
                                KeyCode::Char('r' | 'R') => {
                                    if auth.has_credential() {
                                        start_security_preflight(
                                            &mut state,
                                            &runner,
                                            &ui_tx,
                                            &mut security_cancellation,
                                            &mut security_generation,
                                        );
                                    } else {
                                        state.open_login(Some(
                                            "Sign in before verifying model attestation.".into(),
                                        ));
                                    }
                                }
                                KeyCode::Char('s' | 'S')
                                    if matches!(
                                        state.overlay,
                                        Some(Overlay::Security(SecurityView::Raw))
                                    ) =>
                                {
                                    state.open_attestation_save_picker();
                                }
                                KeyCode::Up => {
                                    state.overlay_scroll = state.overlay_scroll.saturating_sub(1);
                                }
                                KeyCode::Down => {
                                    state.overlay_scroll = state.overlay_scroll.saturating_add(1);
                                }
                                KeyCode::PageUp => {
                                    state.overlay_scroll = state.overlay_scroll.saturating_sub(10);
                                }
                                KeyCode::PageDown => {
                                    state.overlay_scroll = state.overlay_scroll.saturating_add(10);
                                }
                                KeyCode::Home => state.overlay_scroll = 0,
                                _ => {}
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
                                    if !state.clear_composer_draft() {
                                        state.focus = Focus::None;
                                    }
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
                                KeyCode::Char('/') if state.focus != Focus::Composer => {
                                    state.begin_slash_input();
                                }
                                KeyCode::Char('n') if state.focus == Focus::Transcript && !state.search_matches.is_empty() => state.next_match(false),
                                KeyCode::Char('N') if state.focus == Focus::Transcript && !state.search_matches.is_empty() => state.next_match(true),
                                KeyCode::Char('j') if state.focus == Focus::Transcript => state.move_selection(true),
                                KeyCode::Char('k') if state.focus == Focus::Transcript => state.move_selection(false),
                                KeyCode::Char('v') if state.focus == Focus::Transcript && state.selected.is_some() => {
                                    state.overlay = state.selected.map(Overlay::Entry);
                                    state.overlay_scroll = 0;
                                }
                                KeyCode::Tab
                                    if state.focus == Focus::Composer
                                        && state.input.starts_with('/')
                                        && state.input.is_cursor_at_end() =>
                                {
                                    state.complete_slash();
                                }
                                KeyCode::Tab => {
                                    state.focus = if state.focus == Focus::Composer { Focus::Transcript } else { Focus::Composer };
                                    if state.focus == Focus::Transcript && state.selected.is_none() {
                                        state.selected = state.entries.len().checked_sub(1);
                                    }
                                }
                                KeyCode::Enter
                                    if state.focus == Focus::Composer
                                        && key.modifiers.intersects(
                                            KeyModifiers::SHIFT | KeyModifiers::ALT,
                                        ) =>
                                {
                                    state.reset_slash_completion();
                                    state.input.insert_char('\n');
                                }
                                KeyCode::Char('j')
                                    if state.focus == Focus::Composer
                                        && key.modifiers == KeyModifiers::CONTROL =>
                                {
                                    state.reset_slash_completion();
                                    state.input.insert_char('\n');
                                }
                                KeyCode::Char('a')
                                    if state.focus == Focus::Composer
                                        && key.modifiers == KeyModifiers::CONTROL =>
                                {
                                    state.reset_slash_completion();
                                    state.input.move_to_line_start();
                                }
                                KeyCode::Char('e')
                                    if state.focus == Focus::Composer
                                        && key.modifiers == KeyModifiers::CONTROL =>
                                {
                                    state.reset_slash_completion();
                                    state.input.move_to_line_end();
                                }
                                KeyCode::Enter if state.focus == Focus::Transcript => state.toggle_selected(),
                                KeyCode::Enter
                                    if !state.running && state.should_accept_slash_selection() =>
                                {
                                    state.accept_slash_selection();
                                }
                                KeyCode::Enter if !state.running && !state.input.trim().is_empty() => {
                                    let prompt = state.input.take_text();
                                    state.reset_slash_completion();
                                    if let Some(command) = slash::parse(&prompt) {
                                        match command {
                                            Err(error) => show_slash_error(&mut state, &error.to_string()),
                                            Ok(SlashCommand::Help) => {
                                                state.push_entry(
                                                    EntryKind::System,
                                                    slash_help_text(),
                                                    false,
                                                );
                                                state.status = "Slash commands".into();
                                            }
                                            Ok(SlashCommand::Permissions) => {
                                                state.open_permissions();
                                            }
                                            Ok(SlashCommand::Model) => {
                                                state.open_model_picker();
                                                if !state.model_catalog_loaded
                                                    && !state.model_catalog_loading
                                                {
                                                    state.model_catalog_loading = true;
                                                    state.status =
                                                        "Loading available models…".into();
                                                    let task_runner = runner.clone();
                                                    let task_tx = ui_tx.clone();
                                                    tokio::spawn(async move {
                                                        let result = task_runner
                                                            .available_models(
                                                                CancellationToken::new(),
                                                            )
                                                            .await;
                                                        let _ = task_tx
                                                            .send(UiMessage::ModelsLoaded(result))
                                                            .await;
                                                    });
                                                }
                                            }
                                            Ok(SlashCommand::Resume) => {
                                                match store.as_ref() {
                                                    Some(store) => match store.list(false) {
                                                        Ok(sessions) => {
                                                            let current = session_id.to_string();
                                                            let sessions = sessions
                                                                .into_iter()
                                                                .filter(|summary| {
                                                                    summary.id != current
                                                                        && (summary.cwd == cwd
                                                                            || summary
                                                                                .cwd
                                                                                .canonicalize()
                                                                                .is_ok_and(|path| path == cwd))
                                                                })
                                                                .collect();
                                                            state.open_resume_picker(sessions);
                                                        }
                                                        Err(error) => show_slash_error(
                                                            &mut state,
                                                            &error.to_string(),
                                                        ),
                                                    },
                                                    None => show_slash_error(
                                                        &mut state,
                                                        "session resume requires a durable store",
                                                    ),
                                                }
                                            }
                                            Ok(SlashCommand::Delete) => {
                                                match store.as_ref() {
                                                    Some(store) => match store.list(true) {
                                                        Ok(sessions) => {
                                                            let current = session_id.to_string();
                                                            state.open_delete_picker(
                                                                sessions
                                                                    .into_iter()
                                                                    .filter(|summary| summary.id != current)
                                                                    .collect(),
                                                            );
                                                        }
                                                        Err(error) => show_slash_error(
                                                            &mut state,
                                                            &error.to_string(),
                                                        ),
                                                    },
                                                    None => show_slash_error(
                                                        &mut state,
                                                        "session deletion requires a durable store",
                                                    ),
                                                }
                                            }
                                            Ok(SlashCommand::Thinking(level)) => {
                                                match runner.set_thinking_level(&session_id, level).await {
                                                    Ok(()) => match runtime.dispatch(AppCommand::ChangeThinkingLevel {
                                                        session_id: session_id.clone(),
                                                        level,
                                                    }).await {
                                                        Ok(events) => {
                                                            if let Some(store) = &store {
                                                                store.append_all(&events)?;
                                                            }
                                                            for event in events {
                                                                state.apply(&event.event);
                                                            }
                                                        }
                                                        Err(error) => show_slash_error(&mut state, &error.to_string()),
                                                    },
                                                    Err(error) => show_slash_error(&mut state, &error.to_string()),
                                                }
                                            }
                                            Ok(SlashCommand::Login) => {
                                                state.open_login(None);
                                            }
                                            Ok(SlashCommand::Logout) => {
                                                if let Some(token) = auth_cancellation.take() {
                                                    token.cancel();
                                                }
                                                if let Some(token) = security_cancellation.take() {
                                                    token.cancel();
                                                }
                                                security_generation =
                                                    security_generation.wrapping_add(1);
                                                match auth.logout() {
                                                    Ok(()) => {
                                                        state.overlay = None;
                                                        state.reset_security();
                                                        state.status = "Signed out of Axiom".into();
                                                        state.status_tone = StatusTone::Neutral;
                                                        state.push_entry(
                                                            EntryKind::System,
                                                            "AXIOM ACCOUNT\nSigned out. The saved Axiom API key was removed from this computer."
                                                                .into(),
                                                            false,
                                                        );
                                                    }
                                                    Err(error) => show_slash_error(
                                                        &mut state,
                                                        &error.to_string(),
                                                    ),
                                                }
                                            }
                                            Ok(SlashCommand::Account) => {
                                                state.overlay = Some(Overlay::Auth(
                                                    AuthOverlay::Checking {
                                                        label: "Checking Axiom authorization…",
                                                    },
                                                ));
                                                let validation_auth = auth.clone();
                                                let validation_tx = ui_tx.clone();
                                                tokio::spawn(async move {
                                                    let status = validation_auth.validate().await;
                                                    let _ = validation_tx
                                                        .send(UiMessage::AuthValidated(
                                                            ValidationPurpose::Account,
                                                            status,
                                                        ))
                                                        .await;
                                                });
                                            }
                                            Ok(SlashCommand::Security) => {
                                                state.open_security();
                                                if state.security_evidence.is_none()
                                                    && security_cancellation.is_none()
                                                    && auth.has_credential()
                                                {
                                                    start_security_preflight(
                                                        &mut state,
                                                        &runner,
                                                        &ui_tx,
                                                        &mut security_cancellation,
                                                        &mut security_generation,
                                                    );
                                                }
                                            }
                                            Ok(SlashCommand::Refresh) => {
                                                if auth.has_credential() {
                                                    start_security_preflight(
                                                        &mut state,
                                                        &runner,
                                                        &ui_tx,
                                                        &mut security_cancellation,
                                                        &mut security_generation,
                                                    );
                                                    state.status =
                                                        "Refreshing attestation…".into();
                                                    state.status_tone = StatusTone::Active;
                                                } else {
                                                    state.open_login(Some(
                                                        "Sign in before refreshing attestation."
                                                            .into(),
                                                    ));
                                                }
                                            }
                                            Ok(SlashCommand::Compact { focus }) => {
                                                state.running = true;
                                                state.compacting = true;
                                                state.status = "Compacting conversation context…".into();
                                                state.status_tone = StatusTone::Active;
                                                let token = CancellationToken::new();
                                                cancellation = Some(token.clone());
                                                let task_runner = runner.clone();
                                                let task_tx = ui_tx.clone();
                                                let compact_session = session_id.clone();
                                                tokio::spawn(async move {
                                                    let (event_tx, mut event_rx) =
                                                        mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
                                                    let forwarding = task_tx.clone();
                                                    let forward = tokio::spawn(async move {
                                                        while let Some(event) = event_rx.recv().await {
                                                            let _ = forwarding
                                                                .send(UiMessage::Event(event))
                                                                .await;
                                                        }
                                                    });
                                                    let result = task_runner
                                                        .compact(
                                                            &compact_session,
                                                            focus,
                                                            event_tx,
                                                            token,
                                                        )
                                                        .await;
                                                    let _ = forward.await;
                                                    let _ = task_tx
                                                        .send(UiMessage::CompactionDone(result))
                                                        .await;
                                                });
                                            }
                                        }
                                        continue;
                                    }
                                    if startup_auth_pending {
                                        state.input.set_text(prompt);
                                        state.status = "Checking Axiom authorization…".into();
                                        state.status_tone = StatusTone::Waiting;
                                        continue;
                                    }
                                    if security_cancellation.is_some() {
                                        state.input.set_text(prompt);
                                        state.status = "Please wait…".into();
                                        state.status_tone = StatusTone::Waiting;
                                        continue;
                                    }
                                    if !auth.has_credential() && !uses_deterministic_test_runner() {
                                        state.input.set_text(prompt);
                                        state.open_login(Some(
                                            "Sign in before starting this task. Your draft is still here."
                                                .into(),
                                        ));
                                        continue;
                                    }
                                    let turn_id = runtime.turn_id();
                                    let should_generate_title = !state.has_prompt && store.is_some();
                                    let title_prompt = prompt.clone();
                                    let title_session = session_id.clone();
                                    let submitted = runtime.dispatch(AppCommand::SubmitPrompt {
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        text: prompt.clone(),
                                    }).await?;
                                    if let Some(store) = &store {
                                        store.append_all(&submitted)?;
                                        store.set_last_used_model(&state.model)?;
                                        store.set_last_used_thinking(state.thinking)?;
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
                                        permission_profile: state.profile,
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
                                    if should_generate_title {
                                        let title_runner = runner.clone();
                                        let title_tx = ui_tx.clone();
                                        tokio::spawn(async move {
                                            let result = title_runner
                                                .generate_title(
                                                    &title_session,
                                                    &title_prompt,
                                                    CancellationToken::new(),
                                                )
                                                .await;
                                            let _ = title_tx
                                                .send(UiMessage::TitleGenerated(
                                                    title_session,
                                                    result,
                                                ))
                                                .await;
                                        });
                                    }
                                }
                                KeyCode::Char(character) if state.focus == Focus::Composer && (!state.running || key.modifiers.is_empty()) => {
                                    state.reset_slash_completion();
                                    state.input.insert_char(character);
                                }
                                KeyCode::Backspace if state.focus == Focus::Composer => {
                                    state.reset_slash_completion();
                                    state.input.backspace();
                                }
                                KeyCode::Delete if state.focus == Focus::Composer => {
                                    state.reset_slash_completion();
                                    state.input.delete_forward();
                                }
                                KeyCode::Left if state.focus == Focus::Composer => {
                                    state.reset_slash_completion();
                                    state.input.move_left();
                                }
                                KeyCode::Right if state.focus == Focus::Composer => {
                                    state.reset_slash_completion();
                                    state.input.move_right();
                                }
                                KeyCode::Home
                                    if state.focus == Focus::Composer
                                        && key.modifiers == KeyModifiers::CONTROL =>
                                {
                                    state.reset_slash_completion();
                                    state.input.move_to_document_start();
                                }
                                KeyCode::End
                                    if state.focus == Focus::Composer
                                        && key.modifiers == KeyModifiers::CONTROL =>
                                {
                                    state.reset_slash_completion();
                                    state.input.move_to_document_end();
                                }
                                KeyCode::Home if state.focus == Focus::Composer => {
                                    state.reset_slash_completion();
                                    state.input.move_to_line_start();
                                }
                                KeyCode::End if state.focus == Focus::Composer => {
                                    state.reset_slash_completion();
                                    state.input.move_to_line_end();
                                }
                                KeyCode::Up if state.can_navigate_slash() => {
                                    state.move_slash_selection(-1);
                                }
                                KeyCode::Down if state.can_navigate_slash() => {
                                    state.move_slash_selection(1);
                                }
                                KeyCode::Up if state.focus == Focus::Composer => {
                                    let (width, _) = crossterm::terminal::size()?;
                                    state.reset_slash_completion();
                                    state.input.move_up(composer_text_width(width));
                                }
                                KeyCode::Down if state.focus == Focus::Composer => {
                                    let (width, _) = crossterm::terminal::size()?;
                                    state.reset_slash_completion();
                                    state.input.move_down(composer_text_width(width));
                                }
                                KeyCode::Up => {
                                    if state.focus == Focus::Transcript {
                                        state.move_selection(false);
                                    } else {
                                        let (width, height) = crossterm::terminal::size()?;
                                        scroll_transcript_up(
                                            &mut state,
                                            Rect::new(0, 0, width, height),
                                            3,
                                        );
                                    }
                                }
                                KeyCode::Down => {
                                    if state.focus == Focus::Transcript {
                                        state.move_selection(true);
                                    } else {
                                        let (width, height) = crossterm::terminal::size()?;
                                        scroll_transcript_down(
                                            &mut state,
                                            Rect::new(0, 0, width, height),
                                            3,
                                        );
                                    }
                                }
                                KeyCode::End => {
                                    state.follow_output = true;
                                    state.scroll = 0;
                                }
                                _ => {}
                            }
                        }
                    }
                    Event::Paste(text) => {
                        let text = sanitize_terminal_text(&text);
                        if matches!(
                            state.overlay,
                            Some(Overlay::Auth(AuthOverlay::Paste { .. }))
                        ) {
                            state.auth_secret_push(&text.replace('\n', " "));
                        } else if matches!(
                            state.overlay,
                            Some(Overlay::SaveAttestation(SaveAttestationPicker {
                                focus: SavePickerFocus::Filename,
                                ..
                            }))
                        ) {
                            state.push_attestation_filename(&text.replace(['\n', '\r'], ""));
                        } else if pending_questions.is_some() {
                            state.answer_input.push_str(&text.replace('\n', " "));
                        } else if pending_approval.is_none()
                            && state.focus == Focus::Composer
                            && !state.running
                        {
                            state.reset_slash_completion();
                            state.input.insert_str(&text);
                        }
                    }
                    _ => {}
                }
            }
            message = ui_rx.recv() => {
                let Some(message) = message else { break };
                match message {
                    UiMessage::Event(event) => {
                        if matches!(event, AppEvent::SecurityStatusChanged { .. }) {
                            // Security is live process state, not resumable
                            // conversation state.
                            state.apply(&event);
                        } else {
                            let envelope = runtime.emit(session_id.clone(), event.clone()).await?;
                            if let Some(store) = &store {
                                store.append(&envelope)?;
                            }
                            state.apply(&event);
                        }
                    }
                    UiMessage::SecurityPreflightDone(generation, result)
                        if generation == security_generation =>
                    {
                        security_cancellation = None;
                        match result {
                            Ok(SecurityVerification {
                                status:
                                    security @ (crate::app::SecurityStatus::Verified
                                    | crate::app::SecurityStatus::UnattestedDevelopment),
                                evidence,
                            }) => {
                                let manually_refreshed =
                                    state.status == "Refreshing attestation…";
                                state.security_evidence = evidence;
                                state.apply(&AppEvent::SecurityStatusChanged { state: security });
                                if manually_refreshed {
                                    state.status = "Attestation refreshed".into();
                                    state.status_tone = StatusTone::Success;
                                } else if state.status == "Please wait…"
                                    || state.status.starts_with("Security verification")
                                {
                                    state.status = "Ready".into();
                                    state.status_tone = StatusTone::Success;
                                }
                            }
                            Ok(_) => {
                                state.security_evidence = None;
                                state.apply(&AppEvent::SecurityStatusChanged {
                                    state: crate::app::SecurityStatus::Failed,
                                });
                                state.status =
                                    "Security verification returned no final verdict".into();
                                state.status_tone = StatusTone::Error;
                            }
                            Err(AxiomError::Cancelled) => {}
                            Err(error) => {
                                state.security_evidence = None;
                                state.apply(&AppEvent::SecurityStatusChanged {
                                    state: crate::app::SecurityStatus::Failed,
                                });
                                let detail = sanitize_terminal_text(&error.to_string());
                                state.status = if is_attestation_failure(&detail) {
                                    "Attestation failed · Run /refresh or /security for details"
                                        .into()
                                } else {
                                    format!("Security verification failed: {detail}")
                                };
                                state.status_tone = StatusTone::Error;
                            }
                        }
                    }
                    UiMessage::SecurityPreflightDone(_, _) => {}
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
                        state.interaction = None;
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
                    UiMessage::CompactionDone(result) => {
                        state.running = false;
                        state.compacting = false;
                        cancellation = None;
                        match result {
                            Ok(result) => {
                                match runtime.dispatch(AppCommand::RecordContextCompaction {
                                    session_id: session_id.clone(),
                                    summary: result.summary,
                                    messages_before: result.messages_before,
                                }).await {
                                    Ok(events) => {
                                        if let Some(store) = &store {
                                            store.append_all(&events)?;
                                        }
                                        for event in events {
                                            state.apply(&event.event);
                                        }
                                    }
                                    Err(error) => show_slash_error(&mut state, &error.to_string()),
                                }
                            }
                            Err(AxiomError::Cancelled) => {
                                state.status = "Compaction cancelled".into();
                                state.status_tone = StatusTone::Cancelled;
                            }
                            Err(error) => show_slash_error(&mut state, &error.to_string()),
                        }
                    }
                    UiMessage::ModelsLoaded(result) => {
                        state.model_catalog_loading = false;
                        state.model_catalog_loaded = true;
                        state.reset_slash_completion();
                        match result {
                            Ok(mut models) => {
                                models.retain(|model| !model.trim().is_empty());
                                models.sort();
                                models.dedup();
                                state.model_candidates = models;
                                if state.model_candidates.is_empty() {
                                    state.status = "Provider returned no available models".into();
                                } else {
                                    state.align_model_picker_to_current();
                                    state.status = format!(
                                        "Choose from {} available model(s)",
                                        state.model_candidates.len()
                                    );
                                }
                            }
                            Err(error) => {
                                state.model_catalog_loaded = false;
                                state.model_candidates.clear();
                                state.status = format!(
                                    "Could not load available models: {error} · Esc to close"
                                );
                            }
                        }
                    }
                    UiMessage::TitleGenerated(titled_session, result) => {
                        match (store.as_ref(), result) {
                            (Some(store), Ok(title)) => {
                                if let Err(error) = store.set_title_if_absent(&titled_session, &title)
                                {
                                    tracing::warn!(
                                        session_id = %titled_session,
                                        %error,
                                        "could not persist generated session title"
                                    );
                                }
                            }
                            (_, Err(error)) => {
                                tracing::warn!(
                                    session_id = %titled_session,
                                    %error,
                                    "background session title generation failed"
                                );
                            }
                            (None, Ok(_)) => {}
                        }
                    }
                    UiMessage::Approval(request, response) => {
                        if pending_approval.is_some() || pending_questions.is_some() {
                            let _ = response.send(ApprovalResponse::deny());
                            state.status = "Overlapping interaction denied safely".into();
                        } else {
                            state.status = "Waiting for your permission decision".into();
                            state.interaction = Some(InteractionView::Approval(request.clone()));
                            pending_approval = Some((request, response));
                        }
                    }
                    UiMessage::Questions(request, response) => {
                        if pending_questions.is_some() || pending_approval.is_some() {
                            let _ = response.send(Err(AxiomError::InvalidTransition(
                                "another user interaction is already pending".into(),
                            )));
                        } else {
                            let pending = PendingQuestions {
                                request: request.clone(),
                                index: 0,
                                answers: BTreeMap::new(),
                                drafts: BTreeMap::new(),
                                response,
                            };
                            state.answer_input.clear();
                            state.answer_prompt = Some(question_status(&pending));
                            state.interaction = Some(InteractionView::Questions {
                                request,
                                index: 0,
                            });
                            state.focus = Focus::Composer;
                            state.status = state.answer_prompt.clone().unwrap_or_default();
                            pending_questions = Some(pending);
                        }
                    }
                    UiMessage::BrowserLoginStarted(result) => match *result {
                        Ok(login) => {
                            let user_code = login.user_code().to_owned();
                            let authorization_url = login.authorization_url().to_owned();
                            let browser_opened = login.browser_opened();
                            state.overlay = Some(Overlay::Auth(AuthOverlay::Browser {
                                user_code,
                                authorization_url,
                                browser_opened,
                            }));
                            state.status = "Waiting for browser approval…".into();
                            state.status_tone = StatusTone::Waiting;
                            if let Some(token) = auth_cancellation.clone() {
                                let completion_tx = ui_tx.clone();
                                tokio::spawn(async move {
                                    let result = login.complete(token).await;
                                    let _ = completion_tx
                                        .send(UiMessage::AuthCompleted(result))
                                        .await;
                                });
                            }
                        }
                        Err(AxiomError::Cancelled) => {}
                        Err(error) => {
                            auth_cancellation = None;
                            state.open_login(Some(error.to_string()));
                        }
                    },
                    UiMessage::AuthCompleted(result) => {
                        auth_cancellation = None;
                        match result {
                            Ok(account) => {
                                let detail = account_status_text(&account);
                                state.reset_security();
                                state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                    status: format!("Signed in successfully.\n\n{detail}"),
                                }));
                                state.status = "Ready".into();
                                state.status_tone = StatusTone::Success;
                                state.push_entry(
                                    EntryKind::System,
                                    "AXIOM ACCOUNT\nSigned in to Axiom.".into(),
                                    false,
                                );
                                start_security_preflight(
                                    &mut state,
                                    &runner,
                                    &ui_tx,
                                    &mut security_cancellation,
                                    &mut security_generation,
                                );
                            }
                            Err(AxiomError::Cancelled) => {}
                            Err(error) => state.open_login(Some(error.to_string())),
                        }
                    }
                    UiMessage::AuthValidated(purpose, status) => {
                        if purpose == ValidationPurpose::Startup {
                            startup_auth_pending = false;
                        }
                        match (purpose, status) {
                            (_, ValidationStatus::Valid(account)) => {
                                let detail = account_status_text(&account);
                                if purpose == ValidationPurpose::Startup {
                                    state.status = "Ready".into();
                                    state.status_tone = StatusTone::Success;
                                    start_security_preflight(
                                        &mut state,
                                        &runner,
                                        &ui_tx,
                                        &mut security_cancellation,
                                        &mut security_generation,
                                    );
                                } else {
                                    if purpose == ValidationPurpose::Login {
                                        state.reset_security();
                                    }
                                    state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                        status: if purpose == ValidationPurpose::Login {
                                            format!("Axiom API key accepted.\n\n{detail}")
                                        } else {
                                            format!("Connected to Axiom.\n\n{detail}")
                                        },
                                    }));
                                    state.status = "Ready".into();
                                    state.status_tone = StatusTone::Success;
                                    if purpose == ValidationPurpose::Login {
                                        start_security_preflight(
                                            &mut state,
                                            &runner,
                                            &ui_tx,
                                            &mut security_cancellation,
                                            &mut security_generation,
                                        );
                                    }
                                }
                            }
                            (ValidationPurpose::Account, ValidationStatus::Missing) => {
                                state.reset_security();
                                state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                    status: "Not signed in. Use /login to connect an Axiom account."
                                        .into(),
                                }));
                                state.status = "Not signed in".into();
                            }
                            (_, ValidationStatus::Missing) => {
                                state.reset_security();
                                state.open_login(Some(
                                    "Connect an Axiom account before starting a task.".into(),
                                ));
                            }
                            (ValidationPurpose::Account, ValidationStatus::Invalid) => {
                                state.reset_security();
                                state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                    status:
                                        "The saved Axiom API key is invalid or revoked. Use /login to reconnect."
                                            .into(),
                                }));
                                state.status = "Axiom API key is invalid".into();
                                state.status_tone = StatusTone::Error;
                            }
                            (_, ValidationStatus::Invalid) => {
                                state.reset_security();
                                state.open_login(Some(
                                    "The saved Axiom API key is invalid or revoked. Sign in again."
                                        .into(),
                                ));
                            }
                            (ValidationPurpose::Account, ValidationStatus::Unavailable(message)) => {
                                state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                    status: message,
                                }));
                                state.status = "Could not validate Axiom authorization".into();
                                state.status_tone = StatusTone::Waiting;
                            }
                            (ValidationPurpose::Login, ValidationStatus::Unavailable(message)) => {
                                state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                    status: format!(
                                        "The key was saved, but Axiom could not be reached to validate it.\n\n{message}"
                                    ),
                                }));
                                state.status = "Authorization saved · validation unavailable".into();
                                state.status_tone = StatusTone::Waiting;
                            }
                            (ValidationPurpose::Startup, ValidationStatus::Unavailable(message)) => {
                                state.status = format!("Ready · login validation unavailable: {message}");
                                state.status_tone = StatusTone::Waiting;
                                start_security_preflight(
                                    &mut state,
                                    &runner,
                                    &ui_tx,
                                    &mut security_cancellation,
                                    &mut security_generation,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(token) = security_cancellation.take() {
        token.cancel();
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

    let rows = main_rows(area, state);
    render_header(frame, state, theme, rows[0]);
    render_transcript(frame, state, theme, rows[1]);
    render_status(frame, state, theme, rows[2]);
    render_composer(frame, state, theme, rows[3]);
    render_hints(frame, state, theme, rows[4]);

    render_slash_suggestions(frame, state, theme, rows[1]);

    if state.interaction.is_some() {
        render_interaction(frame, state, theme);
    }

    if let Some(overlay) = &state.overlay {
        render_overlay(frame, state, overlay, theme);
    }
}

fn render_slash_suggestions(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    if state.focus != Focus::Composer
        || state.running
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
            let row_background = is_selected.then_some(theme.cherry_deep);
            let label_style = Style::default()
                .fg(if is_selected {
                    theme.blossom_pale
                } else {
                    theme.cherry_bright
                })
                .bg(row_background.unwrap_or(theme.surface))
                .add_modifier(Modifier::BOLD);
            let description_style = Style::default()
                .fg(if is_selected { theme.text } else { theme.muted })
                .bg(row_background.unwrap_or(theme.surface));
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
                    .border_style(Style::default().fg(theme.cherry_deep))
                    .style(Style::default().bg(theme.surface)),
            )
            .style(Style::default().bg(theme.surface)),
        popup,
    );
}

fn composer_text_width(terminal_width: u16) -> u16 {
    terminal_width
        .saturating_sub(GUTTER.saturating_mul(2).saturating_add(5))
        .max(1)
}

fn composer_content_height(area: Rect, state: &TuiState) -> u16 {
    if state.interaction.is_some() || state.answer_prompt.is_some() {
        return 1;
    }
    let proportional_cap = (area.height / 4).max(1);
    let transcript_safe_cap = area.height.saturating_sub(9).max(1);
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

fn main_rows(area: Rect, state: &TuiState) -> [Rect; 5] {
    let composer_height = composer_content_height(area, state).saturating_add(2);
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

fn transcript_max_scroll(state: &TuiState, terminal_area: Rect) -> u16 {
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

fn scroll_transcript_up(state: &mut TuiState, terminal_area: Rect, amount: u16) {
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

fn scroll_transcript_down(state: &mut TuiState, terminal_area: Rect, amount: u16) {
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
fn inset_horizontal(area: Rect, inset: u16) -> Option<Rect> {
    let width = area.width.checked_sub(inset.saturating_mul(2))?;
    (width > 0).then(|| Rect {
        x: area.x.saturating_add(inset),
        y: area.y,
        width,
        height: area.height,
    })
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

    let chip = if state.security == "VERIFYING" {
        format!(" {} VERIFYING ", state.blossom_glyph(false))
    } else {
        format!(" {} ", state.security)
    };
    let chip_width = display_width(&chip);
    let model = state.model.clone();
    let thinking = state.thinking.to_string();
    let profile = state.profile.label().to_owned();
    let tail_width =
        2 + display_width(&model) + 3 + display_width(&thinking) + 3 + display_width(&profile);
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
        right.push(Span::styled(thinking, Style::default().fg(theme.muted)));
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

fn render_transcript(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
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

fn render_startup(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let horizontal_margin = if area.width >= 60 { 8 } else { 2 };
    let width = area.width.saturating_sub(horizontal_margin).clamp(3, 104);
    let requested_height = if area.height >= 13 {
        13
    } else if area.height >= 10 {
        10
    } else {
        7
    };
    let height = requested_height.min(area.height.saturating_sub(2)).max(3);
    let panel = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border))
        .style(Style::default().bg(theme.base));
    let inner = block.inner(panel);
    frame.render_widget(block, panel);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let use_detailed_art = !state.options.ascii && inner.width >= 56 && inner.height >= 11;
    let use_compact_art = !state.options.ascii && inner.width >= 46 && inner.height >= 8;
    let use_ascii_art = state.options.ascii && inner.width >= 46 && inner.height >= 7;
    if !use_detailed_art && !use_compact_art && !use_ascii_art {
        let mut lines = Vec::new();
        let content_rows = if inner.height >= 3 { 3 } else { 1 };
        lines.extend(std::iter::repeat_n(
            Line::default(),
            usize::from(inner.height.saturating_sub(content_rows) / 2),
        ));
        lines.push(Line::from(vec![
            Span::styled(
                state.glyph("✿  AXIOM CLI", "*  AXIOM CLI"),
                Style::default()
                    .fg(theme.cherry_bright)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(theme.muted),
            ),
        ]));
        if inner.height >= 3 {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                state.splash_message,
                Style::default().fg(theme.blossom_pale),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .alignment(Alignment::Center)
                .style(Style::default().bg(theme.base)),
            inner,
        );
        return;
    }

    let columns = Layout::horizontal([
        Constraint::Percentage(35),
        Constraint::Length(1),
        Constraint::Min(24),
    ])
    .split(inner);
    let art: &[&str] = if use_ascii_art {
        &STARTUP_ASCII_ART
    } else if use_detailed_art {
        &STARTUP_BLOSSOM_ART
    } else {
        &STARTUP_COMPACT_BLOSSOM_ART
    };
    let art_height = u16::try_from(art.len()).unwrap_or(columns[0].height);
    let art_top = usize::from(columns[0].height.saturating_sub(art_height) / 2);
    let mut art_lines = vec![Line::default(); art_top];
    art_lines.extend(
        art.iter()
            .map(|line| Line::from(Span::styled(*line, Style::default().fg(theme.cherry)))),
    );
    frame.render_widget(
        Paragraph::new(art_lines)
            .alignment(Alignment::Center)
            .style(Style::default().bg(theme.base)),
        columns[0],
    );

    let content_top = usize::from(columns[2].height.saturating_sub(5) / 2);
    let mut content = vec![Line::default(); content_top];
    content.extend([
        Line::from(vec![
            Span::styled(
                "AXIOM CLI",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(theme.muted),
            ),
        ]),
        Line::default(),
        Line::from(Span::styled(
            state.splash_message,
            Style::default()
                .fg(theme.cherry_bright)
                .add_modifier(Modifier::BOLD),
        )),
        Line::default(),
        Line::from(Span::styled(
            if use_detailed_art {
                "Private AI for the work in front of you."
            } else {
                "Private AI. Kept private."
            },
            Style::default().fg(theme.muted),
        )),
    ]);
    frame.render_widget(
        Paragraph::new(content).style(Style::default().bg(theme.base)),
        columns[2],
    );
}

fn render_status(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(inner) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let status_color = if state.exit_armed {
        theme.warning
    } else {
        match state.status_tone {
            StatusTone::Neutral => theme.muted,
            StatusTone::Active => theme.cherry_bright,
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

fn render_composer(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
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
                .fg(if focused {
                    theme.cherry_bright
                } else {
                    theme.muted
                })
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
    let body = if state.input.is_empty() && !focused {
        "Tab to type".to_owned()
    } else {
        view.lines.join("\n")
    };
    frame.render_widget(
        Paragraph::new(body).style(
            Style::default()
                .fg(if state.input.is_empty() && !focused {
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

/// The editable viewport inside the composer slab: one column for the accent
/// bar, one of padding, and one blank row above and below the text.
fn composer_input_area(area: Rect) -> Option<Rect> {
    let width = area.width.checked_sub(3)?;
    (width > 0 && area.height >= 3).then(|| Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width,
        height: area.height.saturating_sub(2),
    })
}

fn render_hints(frame: &mut Frame<'_>, state: &TuiState, theme: Theme, area: Rect) {
    let Some(inner) = inset_horizontal(area, GUTTER) else {
        return;
    };
    let separator = if state.options.ascii { " | " } else { " · " };
    let mut spans = Vec::new();
    let hints: Vec<(&str, &str)> = match &state.interaction {
        Some(InteractionView::Approval(request)) => {
            let mut hints = vec![("y", "allow once")];
            if request.allow_session_grants {
                hints.push(("e", "exact/session"));
            }
            if request.suggested_prefix_scope.is_some() {
                hints.push(("p", "scope/session"));
            }
            hints.push(("n", "deny"));
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
        None => vec![
            ("Enter", "send"),
            ("Alt+Enter", "newline"),
            ("Tab", "focus"),
            ("j/k", "scroll"),
            ("Ctrl+F", "search"),
            ("v", "view"),
            ("?", "help"),
        ],
    };
    for (index, (key, label)) in hints.into_iter().enumerate() {
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

fn render_interaction(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(interaction) = &state.interaction else {
        return;
    };
    let (title, body, editing) = match interaction {
        InteractionView::Approval(request) => {
            let mut choices = vec!["y  Allow this operation once".to_owned()];
            if request.allow_session_grants {
                choices.push("e  Allow this exact operation for this session".into());
            }
            if let Some(scope) = &request.suggested_prefix_scope {
                choices.push(format!("p  Allow {scope} for this session"));
            }
            choices.push("n  Deny this operation".into());
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
                        Style::default()
                            .fg(theme.cherry_bright)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.cherry))
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

fn render_overlay(frame: &mut Frame<'_>, state: &TuiState, overlay: &Overlay, theme: Theme) {
    if let Overlay::Security(view) = overlay {
        render_security_overlay(frame, state, *view, theme);
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
        Overlay::Help => {
            let rows = u16::try_from(body.lines().count()).unwrap_or(u16::MAX);
            fitted_rect(88, rows.saturating_add(2), frame.area())
        }
        Overlay::Entry(_) => centered_rect(88, 82, frame.area()),
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

fn render_permissions_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(Overlay::Permissions { tab, selected }) = &state.overlay else {
        return;
    };
    let area = fitted_rect(82, 23, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Permissions ",
            Style::default()
                .fg(theme.cherry_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.cherry))
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
    let tab_span = |label: &'static str, active: bool| {
        Span::styled(
            if active {
                format!(" {label} ")
            } else {
                format!("  {label}  ")
            },
            if active {
                Style::default()
                    .fg(theme.blossom_pale)
                    .bg(theme.cherry_deep)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.muted)
            },
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            tab_span("Tools", *tab == PermissionTab::Tools),
            Span::raw("  "),
            tab_span("Crypto", *tab == PermissionTab::Crypto),
        ])),
        rows[0],
    );

    match tab {
        PermissionTab::Tools => {
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
                        .fg(theme.blossom_pale)
                        .bg(theme.cherry_deep)
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
        }
        PermissionTab::Crypto => {
            let message_area = Rect::new(
                rows[1].x,
                rows[1].y.saturating_add(rows[1].height / 2),
                rows[1].width,
                1,
            );
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Coming soon!",
                    Style::default()
                        .fg(theme.cherry_bright)
                        .add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Center),
                message_area,
            );
        }
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            truncate_end(
                match tab {
                    PermissionTab::Tools => {
                        "↑/↓ select · Enter apply · Tab or ←/→ switch · Esc cancel"
                    }
                    PermissionTab::Crypto => "Tab or ←/→ switch · Esc close",
                },
                usize::from(rows[2].width),
            ),
            Style::default().fg(theme.muted),
        )),
        rows[2],
    );
}

const MAX_WORKLOAD_DISPLAY_CHARS: usize = 100_000;

fn render_attestation_save_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(Overlay::SaveAttestation(picker)) = &state.overlay else {
        return;
    };
    let area = centered_rect(82, 72, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Save attestation ",
            Style::default()
                .fg(theme.cherry_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.cherry))
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
                    theme.cherry_bright
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
                    theme.cherry_bright
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
                        .fg(theme.blossom_pale)
                        .bg(theme.cherry_deep)
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

fn render_security_overlay(
    frame: &mut Frame<'_>,
    state: &TuiState,
    view: SecurityView,
    theme: Theme,
) {
    let area = centered_rect(88, 86, frame.area());
    frame.render_widget(Clear, area);
    let title = match view {
        SecurityView::Summary => " Security ",
        SecurityView::Workload => " Attested workload ",
        SecurityView::Raw => " Raw app_compose ",
    };
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(theme.cherry_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.cherry))
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
        Span::styled("V", Style::default().fg(theme.cherry_bright)),
        Span::styled(" next view  ·  ", Style::default().fg(theme.muted)),
        Span::styled("R", Style::default().fg(theme.cherry_bright)),
        Span::styled(" verify again  ·  ", Style::default().fg(theme.muted)),
    ];
    if view == SecurityView::Raw && state.security_evidence.is_some() {
        help.extend([
            Span::styled("S", Style::default().fg(theme.cherry_bright)),
            Span::styled(" save  ·  ", Style::default().fg(theme.muted)),
        ]);
    }
    help.push(Span::styled(
        "↑/↓ scroll  ·  Esc close",
        Style::default().fg(theme.muted),
    ));
    frame.render_widget(Paragraph::new(Line::from(help)), rows[1]);
}

fn security_overlay_lines(
    state: &TuiState,
    view: SecurityView,
    theme: Theme,
) -> Vec<Line<'static>> {
    if state.security == "VERIFYING" {
        return vec![
            Line::from(vec![
                Span::styled(
                    format!("{}  ", state.blossom_glyph(false)),
                    Style::default().fg(state.blossom_color(theme, 0)),
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
        } else if state.security == "UNATTESTED DEV" {
            "This process is using the explicitly enabled unattested development transport."
        } else {
            "No attestation evidence is available. Press R to verify now."
        };
        return vec![Line::from(Span::styled(
            message,
            Style::default().fg(if state.security == "SECURITY FAILED" {
                theme.error
            } else {
                theme.warning
            }),
        ))];
    };

    match view {
        SecurityView::Summary => security_summary_lines(evidence, theme),
        SecurityView::Workload => workload_lines(evidence, false, theme),
        SecurityView::Raw => workload_lines(evidence, true, theme),
    }
}

fn security_summary_lines(evidence: &SecurityEvidence, theme: Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    security_field(
        &mut lines,
        "Status",
        "Cryptographically verified",
        theme.success,
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
        "Verified",
        &format_verification_time(evidence.verified_at_unix_seconds),
        theme.text,
        theme,
    );
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
                } else {
                    theme.error
                }),
            ),
            Span::styled(check.label.clone(), Style::default().fg(theme.text)),
            Span::styled(
                format!("  {}", check.status),
                Style::default().fg(theme.muted),
            ),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(section_line("Workload", theme));
    if let Some(value) = evidence_claim(evidence, "intel_mr_td") {
        security_field(&mut lines, "TD measurement", value, theme.text, theme);
    }
    if let Some(value) = evidence_claim(evidence, "workload_manifest_sha256") {
        security_field(&mut lines, "Manifest SHA-256", value, theme.text, theme);
    }

    if let Some(manifest) = evidence.workload_manifest.as_deref() {
        let parsed = parse_workload_manifest(manifest);
        for (key, value) in parsed.metadata {
            security_field(
                &mut lines,
                &humanize_manifest_key(&key),
                &value,
                theme.text,
                theme,
            );
        }
        if !parsed.services.is_empty() {
            lines.push(Line::from(""));
            lines.push(section_line("Attested services", theme));
            for service in parsed.services {
                lines.push(Line::from(vec![
                    Span::styled("• ", Style::default().fg(theme.cherry_bright)),
                    Span::styled(
                        service.name,
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        service
                            .image
                            .map_or_else(String::new, |image| format!("  {image}")),
                        Style::default().fg(theme.muted),
                    ),
                ]));
            }
        }
    } else {
        lines.push(Line::from(Span::styled(
            "The provider did not retain a workload manifest.",
            Style::default().fg(theme.warning),
        )));
    }

    lines.push(Line::from(""));
    lines.push(section_line("Endpoint bindings", theme));
    security_field(
        &mut lines,
        "Model key SHA-256",
        &evidence.model_key_fingerprint,
        theme.text,
        theme,
    );
    security_field(
        &mut lines,
        "TLS SPKI SHA-256",
        evidence
            .tls_spki_fingerprint
            .as_deref()
            .unwrap_or("Unavailable"),
        theme.text,
        theme,
    );
    lines
}

fn workload_lines(evidence: &SecurityEvidence, raw: bool, theme: Theme) -> Vec<Line<'static>> {
    let Some(manifest) = evidence.workload_manifest.as_deref() else {
        return vec![Line::from(Span::styled(
            "No workload manifest was retained for this provider.",
            Style::default().fg(theme.warning),
        ))];
    };
    let parsed = parse_workload_manifest(manifest);
    let (heading, content) = if raw {
        (
            "Exact provider-supplied app_compose bytes (terminal-sanitized)",
            manifest.to_owned(),
        )
    } else if let Some(compose) = parsed.compose {
        (
            "Docker Compose configuration committed to by the quote",
            compose,
        )
    } else {
        (
            "Workload manifest committed to by the quote",
            manifest.to_owned(),
        )
    };
    let (content, truncated) = bounded_workload_display(&sanitize_terminal_text(&content));
    let mut lines = vec![
        section_line(heading, theme),
        Line::from(Span::styled(
            "This is public attestation evidence. It is not a live process list.",
            Style::default().fg(theme.muted),
        )),
        Line::from(""),
    ];
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

fn security_field(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    value_color: Color,
    theme: Theme,
) {
    lines.push(Line::from(vec![
        Span::styled(format!("{label:<20}  "), Style::default().fg(theme.muted)),
        Span::styled(value.to_owned(), Style::default().fg(value_color)),
    ]));
}

fn section_line(label: &str, theme: Theme) -> Line<'static> {
    Line::from(Span::styled(
        label.to_owned(),
        Style::default()
            .fg(theme.cherry_bright)
            .add_modifier(Modifier::BOLD),
    ))
}

fn evidence_claim<'a>(evidence: &'a SecurityEvidence, name: &str) -> Option<&'a str> {
    evidence
        .provider_claims
        .iter()
        .find(|claim| claim.name == name)
        .map(|claim| claim.value.as_str())
}

fn format_verification_time(timestamp: u64) -> String {
    i64::try_from(timestamp)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map_or_else(
            || format!("Unix timestamp {timestamp}"),
            |time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        )
}

fn attestation_export_filename(evidence: &SecurityEvidence) -> String {
    i64::try_from(evidence.verified_at_unix_seconds)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map_or_else(
            || {
                format!(
                    "axiom-attestation-{}.json",
                    evidence.verified_at_unix_seconds
                )
            },
            |time| format!("axiom-attestation-{}.json", time.format("%Y%m%d-%H%M%S")),
        )
}

fn attestation_picker_directories(directory: &Path) -> Vec<(String, PathBuf)> {
    let mut directories = Vec::new();
    if let Some(parent) = directory.parent() {
        directories.push(("../".into(), parent.to_path_buf()));
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return directories;
    };
    let mut children = entries
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            path.is_dir().then(|| {
                let name = sanitize_terminal_text(&entry.file_name().to_string_lossy());
                (format!("{name}/"), path)
            })
        })
        .collect::<Vec<_>>();
    children.sort_by_key(|(name, _)| name.to_lowercase());
    directories.extend(children);
    directories
}

fn write_attestation_export(
    directory: &Path,
    filename: &str,
    evidence: &SecurityEvidence,
) -> Result<PathBuf> {
    let mut components = Path::new(filename).components();
    let filename_is_safe =
        matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();
    if !filename_is_safe {
        return Err(AxiomError::Config(
            "enter a filename without directory separators".into(),
        ));
    }

    let destination = directory.join(filename);
    let mut encoded = serde_json::to_vec_pretty(evidence)?;
    encoded.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                AxiomError::Config(format!(
                    "{} already exists; choose another filename",
                    destination.display()
                ))
            } else {
                AxiomError::Io(error)
            }
        })?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    Ok(destination)
}

#[derive(Debug, Default)]
struct ParsedWorkloadManifest {
    metadata: Vec<(String, String)>,
    compose: Option<String>,
    services: Vec<WorkloadService>,
}

#[derive(Debug)]
struct WorkloadService {
    name: String,
    image: Option<String>,
}

fn parse_workload_manifest(manifest: &str) -> ParsedWorkloadManifest {
    let Ok(serde_json::Value::Object(object)) = serde_json::from_str(manifest) else {
        return ParsedWorkloadManifest {
            services: parse_compose_services(manifest),
            ..ParsedWorkloadManifest::default()
        };
    };
    let mut parsed = ParsedWorkloadManifest::default();
    for (key, value) in object {
        if key == "docker_compose_file" {
            if let Some(compose) = value.as_str() {
                parsed.services = parse_compose_services(compose);
                parsed.compose = Some(compose.to_owned());
            }
            continue;
        }
        let value = match value {
            serde_json::Value::Null => "null".to_owned(),
            serde_json::Value::String(value) => value,
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            value => serde_json::to_string(&value).unwrap_or_else(|_| "unavailable".into()),
        };
        parsed.metadata.push((key, truncate_end(&value, 160)));
    }
    parsed
}

fn parse_compose_services(compose: &str) -> Vec<WorkloadService> {
    let mut services = Vec::new();
    let mut in_services = false;
    for line in compose.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len().saturating_sub(line.trim_start().len());
        if indent == 0 {
            in_services = trimmed == "services:";
            continue;
        }
        if !in_services {
            continue;
        }
        if indent == 2 && trimmed.ends_with(':') {
            if services.len() >= 128 {
                break;
            }
            services.push(WorkloadService {
                name: trimmed.trim_end_matches(':').to_owned(),
                image: None,
            });
        } else if indent == 4
            && let Some(image) = trimmed.strip_prefix("image:")
            && let Some(service) = services.last_mut()
        {
            service.image = Some(image.trim().trim_matches(['\'', '"']).to_owned());
        }
    }
    services
}

fn humanize_manifest_key(key: &str) -> String {
    let mut words = key.split('_').filter(|word| !word.is_empty());
    let Some(first) = words.next() else {
        return key.to_owned();
    };
    let mut output = first.to_owned();
    if let Some(initial) = output.get_mut(0..1) {
        initial.make_ascii_uppercase();
    }
    for word in words {
        output.push(' ');
        output.push_str(word);
    }
    output
}

fn bounded_workload_display(text: &str) -> (String, bool) {
    if text.chars().count() <= MAX_WORKLOAD_DISPLAY_CHARS {
        return (text.to_owned(), false);
    }
    (
        text.chars().take(MAX_WORKLOAD_DISPLAY_CHARS).collect(),
        true,
    )
}

fn render_auth_overlay(
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
        AuthOverlay::Paste { .. } => 9,
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
        AuthOverlay::Paste {
            kind: AuthPasteKind::ApiKey,
            ..
        } => " Axiom API key ",
        AuthOverlay::Paste {
            kind: AuthPasteKind::CompletionCode,
            ..
        } => " Paste one-time code ",
        AuthOverlay::Checking { .. } => " Checking authorization ",
    };
    let mut lines = Vec::new();
    match overlay {
        AuthOverlay::Menu { selected, message } => {
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
            for (index, label) in ["Continue in browser", "Continue with an Axiom API key"]
                .into_iter()
                .enumerate()
            {
                let marker = if index == *selected { "›" } else { " " };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {marker} "),
                        Style::default().fg(if index == *selected {
                            theme.cherry_bright
                        } else {
                            theme.muted
                        }),
                    ),
                    Span::styled(
                        label,
                        Style::default()
                            .fg(if index == *selected {
                                theme.text
                            } else {
                                theme.muted
                            })
                            .add_modifier(if index == *selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                ]));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "↑/↓ select · Enter continue · Esc close",
                Style::default().fg(theme.muted),
            )));
        }
        AuthOverlay::Starting => {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}  ", state.blossom_glyph(false)),
                    Style::default().fg(state.blossom_color(theme, 0)),
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
                    format!("{}  ", state.blossom_glyph(false)),
                    Style::default().fg(state.blossom_color(theme, 0)),
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
                Style::default()
                    .fg(theme.cherry_bright)
                    .add_modifier(Modifier::BOLD),
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
                    .fg(theme.cherry_bright)
                    .add_modifier(Modifier::UNDERLINED),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "O reopen browser · P paste fallback code · Esc cancel",
                Style::default().fg(theme.muted),
            )));
        }
        AuthOverlay::Paste { kind, input } => {
            lines.push(Line::from(Span::styled(
                if *kind == AuthPasteKind::ApiKey {
                    "Paste your Axiom API key (axm_…). It will be stored in your system credential store when available."
                } else {
                    "Paste the axc_ one-time code shown by the approval page. It can only be exchanged once."
                },
                Style::default().fg(theme.muted),
            )));
            lines.push(Line::from(""));
            let count = input.0.chars().count();
            let visible = "•".repeat(count.min(48));
            let suffix = if count > 48 { "…" } else { "" };
            lines.push(Line::from(Span::styled(
                format!("› {visible}{suffix}_"),
                Style::default().fg(theme.cherry_bright),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter submit · Esc back",
                Style::default().fg(theme.muted),
            )));
        }
        AuthOverlay::Checking { label } => {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}  ", state.blossom_glyph(false)),
                    Style::default().fg(state.blossom_color(theme, 0)),
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
                        Style::default()
                            .fg(theme.cherry)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.border))
                    .padding(Padding::new(2, 2, 1, 1)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_model_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
    let Some(Overlay::ModelPicker { query, selected }) = &state.overlay else {
        return;
    };
    let area = centered_rect(76, 74, frame.area());
    frame.render_widget(Clear, area);
    let block = Block::default()
        .title(Span::styled(
            " Select model ",
            Style::default()
                .fg(theme.cherry_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.cherry))
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
                Style::default()
                    .fg(theme.cherry_bright)
                    .add_modifier(Modifier::BOLD),
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
            let label = truncate_end(
                &format!("{marker} {model}{current}"),
                usize::from(rows[2].width),
            );
            let style = if is_selected {
                Style::default()
                    .fg(theme.blossom_pale)
                    .bg(theme.cherry_deep)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(if is_current {
                    theme.cherry_bright
                } else {
                    theme.text
                })
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

fn render_resume_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
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
            Style::default()
                .fg(theme.cherry_bright)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.cherry))
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
                Style::default()
                    .fg(theme.cherry_bright)
                    .add_modifier(Modifier::BOLD),
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
                    .fg(theme.blossom_pale)
                    .bg(theme.cherry_deep)
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

fn render_delete_picker(frame: &mut Frame<'_>, state: &TuiState, theme: Theme) {
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
            theme.cherry
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
                Style::default()
                    .fg(theme.cherry_bright)
                    .add_modifier(Modifier::BOLD),
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
                    .fg(theme.blossom_pale)
                    .bg(theme.cherry_deep)
                    .add_modifier(Modifier::BOLD)
            } else if marked.contains(&session.id) {
                Style::default().fg(theme.cherry_bright)
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

/// Keeps the leading display columns of `text`, reserving the final column for
/// an ellipsis when truncation is required.
fn truncate_end(text: &str, width: usize) -> String {
    if display_width(text) <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let mut truncated = String::new();
    let mut used = 0_usize;
    for character in text.chars() {
        let character_width = char_width(character);
        if used.saturating_add(character_width) > width - 1 {
            break;
        }
        truncated.push(character);
        used = used.saturating_add(character_width);
    }
    truncated.push('…');
    truncated
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

const fn estimate_streamed_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

// Retained with the dormant counter implementation so reintroducing a token
// surface does not require rebuilding its formatting behavior.
#[allow(dead_code)]
fn format_token_usage(total: u64) -> String {
    let digits = total.to_string();
    let mut reversed = String::with_capacity(digits.len().saturating_add(digits.len() / 3));
    for (index, character) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            reversed.push(',');
        }
        reversed.push(character);
    }
    let grouped = reversed.chars().rev().collect::<String>();
    let noun = if total == 1 { "token" } else { "tokens" };
    format!("{grouped} {noun} used")
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
    if is_attestation_failure(&normalized) {
        "Secure connection could not be verified\nYour request was not sent. Run /refresh to try again or /security for details."
            .into()
    } else if payment_required && insufficient_credit {
        "Out of credits\nVisit axiom.stream to top up your balance.".into()
    } else {
        message.to_owned()
    }
}

fn is_attestation_failure(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("attestationrejected")
        || normalized.contains("attestation rejected")
        || normalized.contains("attestationunavailable")
        || normalized.contains("attestation unavailable")
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
    use axiom_secure_client::{EvidenceCheck, EvidenceClaim, SecurityState};
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn security_evidence() -> SecurityEvidence {
        SecurityEvidence {
            state: SecurityState::Verified,
            provider_id: "near".into(),
            model_id: "deepseek-v4-flash".into(),
            attestation_protocol: "near-tdx-nvidia-v1".into(),
            e2ee_protocol: "near-v2".into(),
            e2ee_encryption_version: 2,
            trust_policy_version: "near-v2-trust-v1".into(),
            verified_at_unix_seconds: 1_777_776_000,
            model_key_fingerprint: "11".repeat(32),
            tls_spki_fingerprint: Some("22".repeat(32)),
            checks: vec![EvidenceCheck {
                id: "intel_tdx".into(),
                label: "Intel TDX".into(),
                status: "UpToDate".into(),
                passed: true,
            }],
            provider_claims: vec![
                EvidenceClaim {
                    name: "intel_mr_td".into(),
                    value: "33".repeat(48),
                },
                EvidenceClaim {
                    name: "workload_manifest_sha256".into(),
                    value: "44".repeat(32),
                },
            ],
            workload_manifest: Some(
                serde_json::json!({
                    "manifest_version": 2,
                    "name": "dstack-nvidia-test",
                    "runner": "docker-compose",
                    "local_key_provider_enabled": false,
                    "docker_compose_file": "services:\n  inference:\n    image: registry.example/inference@sha256:abcd\n  gateway:\n    image: registry.example/gateway@sha256:ef01\n",
                })
                .to_string(),
            ),
            workload_identity_allowlist_configured: false,
            approved_workload_identity: false,
        }
    }

    #[test]
    fn security_inspector_shows_summary_workload_and_exact_manifest_views() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "deepseek-v4-flash".into(),
            PermissionProfile::Confirm,
        );
        state.security = "SECURE";
        state.security_evidence = Some(security_evidence());
        state.open_security();

        let summary = render_symbols(120, 42, &state);
        assert!(summary.contains("Security"));
        assert!(summary.contains("Cryptographically verified"));
        assert!(summary.contains("Intel TDX"));
        assert!(summary.contains("dstack-nvidia-test"));
        assert!(summary.contains("Local key provider enabled  false"));
        assert!(summary.contains("inference"));
        assert!(!summary.contains("Axiom approval"));
        assert!(!summary.contains("allowlist configured"));

        state.cycle_security_view();
        let workload = render_symbols(120, 42, &state);
        assert!(workload.contains("Attested workload"));
        assert!(workload.contains("registry.example/inference@sha256:abcd"));

        state.cycle_security_view();
        let raw = render_symbols(120, 42, &state);
        assert!(raw.contains("Raw app_compose"));
        assert!(raw.contains("manifest_version"));
        assert!(raw.contains("docker_compose_file"));
        assert!(raw.contains("S save"));
    }

    #[test]
    fn raw_attestation_view_opens_a_keyboard_save_picker() {
        let directory = tempfile::tempdir().expect("tempdir");
        fs::create_dir(directory.path().join("exports")).expect("exports directory");
        let mut state = TuiState::new(
            directory.path().to_path_buf(),
            "deepseek-v4-flash".into(),
            PermissionProfile::Confirm,
        );
        state.security = "SECURE";
        state.security_evidence = Some(security_evidence());
        state.overlay = Some(Overlay::Security(SecurityView::Raw));

        state.open_attestation_save_picker();
        let picker = render_symbols(120, 42, &state);
        assert!(picker.contains("Save attestation"));
        assert!(picker.contains("axiom-attestation-"));
        assert!(picker.contains("exports/"));
        assert!(picker.contains("Enter save"));

        state.close_attestation_save_picker();
        assert!(matches!(
            state.overlay,
            Some(Overlay::Security(SecurityView::Raw))
        ));
    }

    #[test]
    fn attestation_export_is_complete_pretty_json_and_never_overwrites() {
        let directory = tempfile::tempdir().expect("tempdir");
        let evidence = security_evidence();
        let destination =
            write_attestation_export(directory.path(), "axiom-attestation.json", &evidence)
                .expect("save evidence");
        let encoded = fs::read_to_string(&destination).expect("read evidence");
        assert!(encoded.ends_with('\n'));
        assert!(encoded.contains("\n  \"workload_manifest\""));
        assert_eq!(
            serde_json::from_str::<SecurityEvidence>(&encoded).expect("decode evidence"),
            evidence
        );

        let overwrite = write_attestation_export(
            directory.path(),
            "axiom-attestation.json",
            &security_evidence(),
        )
        .expect_err("existing evidence must not be overwritten");
        assert!(overwrite.to_string().contains("already exists"));
        assert!(
            write_attestation_export(directory.path(), "../escape.json", &security_evidence())
                .is_err()
        );
    }

    #[test]
    fn ready_status_uses_the_success_tone() {
        let state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        assert_eq!(state.status, "Ready");
        assert_eq!(state.status_tone, StatusTone::Success);
    }

    #[test]
    fn workload_parser_extracts_digest_pinned_compose_services_and_bounds_unicode() {
        let evidence = security_evidence();
        let parsed = parse_workload_manifest(
            evidence
                .workload_manifest
                .as_deref()
                .expect("workload manifest"),
        );
        assert_eq!(parsed.services.len(), 2);
        assert_eq!(parsed.services[0].name, "inference");
        assert_eq!(
            parsed.services[0].image.as_deref(),
            Some("registry.example/inference@sha256:abcd")
        );
        let oversized = "🌸".repeat(MAX_WORKLOAD_DISPLAY_CHARS + 1);
        let (bounded, truncated) = bounded_workload_display(&oversized);
        assert!(truncated);
        assert_eq!(bounded.chars().count(), MAX_WORKLOAD_DISPLAY_CHARS);
        assert!(bounded.ends_with('🌸'));
    }

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
    fn terminal_background_commands_cover_padding_and_restore_the_host_color() {
        assert_eq!(ColorMode::TrueColor.terminal_background(), Some("#101013"));
        assert_eq!(ColorMode::Ansi16.terminal_background(), Some("#000000"));
        assert_eq!(ColorMode::NoColor.terminal_background(), None);

        let mut start = String::new();
        SetAxiomTerminalBackground(Some("#101013"))
            .write_ansi(&mut start)
            .expect("set background sequence");
        assert_eq!(start, "\x1b]11;#101013\x1b\\");

        let mut reset = String::new();
        ResetAxiomTerminalBackground(true)
            .write_ansi(&mut reset)
            .expect("reset background sequence");
        assert_eq!(reset, "\x1b]111\x1b\\");

        let mut no_color = String::new();
        SetAxiomTerminalBackground(None)
            .write_ansi(&mut no_color)
            .expect("no-color sequence");
        ResetAxiomTerminalBackground(false)
            .write_ansi(&mut no_color)
            .expect("no-color reset sequence");
        assert!(no_color.is_empty());
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
    fn slash_command_menu_is_visible_and_filtered_while_typing() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.input = "/thi".into();
        let output = render_symbols(100, 30, &state);
        assert!(output.contains("/thinking"));
        assert!(output.contains("reasoning effort"));
        assert!(!output.contains("/compact ["));
    }

    #[test]
    fn permissions_picker_has_tools_and_crypto_tabs() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.open_permissions();
        let output = render_symbols(100, 30, &state);
        assert!(output.contains("Tools"));
        assert!(output.contains("Crypto"));
        assert!(output.contains("Full Access"));
        assert!(output.contains("one confirmation per use"));
        assert!(!output.contains("Control which tools"));
        assert!(!output.contains("Coming soon"));
        assert!(output.contains("✓ Confirm"));
        assert!(!output.contains("Current"));
        assert_eq!(
            state.selected_permission_profile(),
            Some(PermissionProfile::Confirm)
        );
        let compact = render_symbols(MIN_WIDTH, MIN_HEIGHT, &state);
        assert!(compact.contains("Confirm"));

        state.switch_permission_tab();
        let crypto = render_symbols(100, 30, &state);
        assert!(crypto.contains("Coming soon!"));
        assert!(!crypto.contains("Crypto authority is separate"));
        assert!(!crypto.contains("Wallets, networks"));
        assert_eq!(state.selected_permission_profile(), None);
    }

    #[test]
    fn slash_shortcut_focuses_composer_and_invalid_enter_can_complete() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.focus = Focus::Transcript;
        state.begin_slash_input();
        assert_eq!(state.focus, Focus::Composer);
        assert_eq!(state.input, "/");

        state.input = "/permi".into();
        assert!(state.invalid_slash_has_completion());
        state.complete_slash();
        assert_eq!(state.input, "/permissions");
        assert!(!state.invalid_slash_has_completion());

        state.input = "/not-a-command".into();
        assert!(!state.invalid_slash_has_completion());
    }

    #[test]
    fn repeated_tab_cycles_matching_options_in_declared_order() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.input = "/thinking m".into();

        state.complete_slash();
        assert_eq!(state.input, "/thinking minimal");
        state.complete_slash();
        assert_eq!(state.input, "/thinking medium");
        state.complete_slash();
        assert_eq!(state.input, "/thinking minimal");
    }

    #[test]
    fn arrow_keys_highlight_slash_commands_and_options_before_accepting() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.input = "/".into();
        assert!(state.can_navigate_slash());
        state.move_slash_selection(1);
        assert_eq!(state.input, "/", "navigation must preserve the draft");
        assert!(state.should_accept_slash_selection());
        assert!(render_symbols(100, 30, &state).contains("› /compact"));
        state.accept_slash_selection();
        assert_eq!(state.input, "/compact ");

        state.reset_slash_completion();
        state.input = "/thinking ".into();
        state.move_slash_selection(1);
        state.move_slash_selection(1);
        assert_eq!(state.input, "/thinking ");
        assert!(render_symbols(100, 30, &state).contains("› medium"));
        state.accept_slash_selection();
        assert_eq!(state.input, "/thinking medium");
    }

    #[test]
    fn model_picker_filters_navigates_and_selects_catalog_values() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "grok-code".into(),
            PermissionProfile::Confirm,
        );
        state.model_candidates = vec!["glm-5-2".into(), "grok-code".into(), "qwen-code".into()];
        state.model_catalog_loaded = true;

        state.open_model_picker();
        assert_eq!(state.selected_model().as_deref(), Some("grok-code"));

        state.push_model_picker_search('g');
        assert_eq!(state.selected_model().as_deref(), Some("glm-5-2"));
        state.move_model_picker(1);
        assert_eq!(state.selected_model().as_deref(), Some("grok-code"));

        let output = render_symbols(100, 30, &state);
        assert!(output.contains("Select model"));
        assert!(output.contains("Search"));
        assert!(output.contains("glm-5-2"));
        assert!(output.contains("grok-code"));
        assert!(!output.contains("qwen-code"));
    }

    #[test]
    fn resume_picker_filters_navigates_and_selects_saved_transcripts() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "grok-code".into(),
            PermissionProfile::Confirm,
        );
        let sessions = vec![
            SessionSummary {
                id: "11111111-1111-1111-1111-111111111111".into(),
                title: Some("Renderer repair".into()),
                cwd: PathBuf::from("/tmp/project"),
                origin: "tui".into(),
                profile: "confirm".into(),
                archived: false,
                history_truncated: false,
                updated_at: "2026-08-22T16:42:00Z".into(),
            },
            SessionSummary {
                id: "22222222-2222-2222-2222-222222222222".into(),
                title: Some("Release verification".into()),
                cwd: PathBuf::from("/tmp/project"),
                origin: "tui".into(),
                profile: "workspace".into(),
                archived: false,
                history_truncated: false,
                updated_at: "2026-08-21T10:15:00Z".into(),
            },
        ];

        state.open_resume_picker(sessions);
        assert_eq!(
            state
                .selected_resume_session()
                .expect("selected")
                .title
                .as_deref(),
            Some("Renderer repair")
        );
        state.push_resume_picker_search('r');
        state.push_resume_picker_search('e');
        state.move_resume_picker(1);
        assert_eq!(
            state
                .selected_resume_session()
                .expect("selected")
                .title
                .as_deref(),
            Some("Release verification")
        );

        let output = render_symbols(100, 30, &state);
        assert!(output.contains("Resume transcript"));
        assert!(output.contains("Renderer repair"));
        assert!(output.contains("Release verification"));
        assert!(output.contains("2026-08-22 16:42"));
    }

    #[test]
    fn delete_picker_searches_multiselects_selects_all_and_confirms() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "grok-code".into(),
            PermissionProfile::Confirm,
        );
        let sessions = vec![
            SessionSummary {
                id: "11111111-1111-1111-1111-111111111111".into(),
                title: Some("Renderer repair".into()),
                cwd: PathBuf::from("/tmp/project"),
                origin: "tui".into(),
                profile: "confirm".into(),
                archived: false,
                history_truncated: false,
                updated_at: "2026-08-22T16:42:00Z".into(),
            },
            SessionSummary {
                id: "22222222-2222-2222-2222-222222222222".into(),
                title: Some("Release verification".into()),
                cwd: PathBuf::from("/tmp/other"),
                origin: "tui".into(),
                profile: "workspace".into(),
                archived: true,
                history_truncated: false,
                updated_at: "2026-08-21T10:15:00Z".into(),
            },
        ];

        state.open_delete_picker(sessions);
        state.toggle_delete_picker_session();
        state.move_delete_picker(1);
        state.toggle_delete_picker_session();
        assert_eq!(state.marked_delete_session_ids().expect("IDs").len(), 2);
        state.toggle_all_delete_picker_matches();
        assert!(
            state
                .marked_delete_session_ids()
                .expect("cleared IDs")
                .is_empty()
        );
        state.toggle_all_delete_picker_matches();
        state.begin_delete_confirmation();
        assert!(matches!(
            state.overlay,
            Some(Overlay::DeletePicker {
                confirming: true,
                ..
            })
        ));

        let output = render_symbols(100, 30, &state);
        assert!(output.contains("Delete transcripts"));
        assert!(output.contains("2 selected"));
        assert!(output.contains("cannot be undone"));
        assert!(output.contains("Renderer repair"));
        assert!(output.contains("Release verification"));
    }

    #[test]
    fn composer_status_marker_is_static_and_uses_terminal_outcome_color() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        let turn_id = TurnId::new();
        state.apply(&AppEvent::PromptAccepted {
            turn_id: turn_id.clone(),
            text: "test status".into(),
        });
        state.apply(&AppEvent::TurnStarted {
            turn_id: turn_id.clone(),
        });
        let working = render_symbols(100, 30, &state);
        state.tick_animation();
        let next_frame = render_symbols(100, 30, &state);
        assert!(working.contains("● Working"));
        assert!(next_frame.contains("● Working"));

        state.apply(&AppEvent::TurnCompleted { turn_id });
        assert_eq!(state.status_tone, StatusTone::Success);
        assert!(render_symbols(100, 30, &state).contains("● Task complete"));
        state.apply(&AppEvent::ErrorRaised {
            turn_id: None,
            message: "failed".into(),
        });
        assert_eq!(state.status_tone, StatusTone::Error);
        assert!(render_symbols(100, 30, &state).contains("● Error"));
    }

    #[test]
    fn compaction_has_a_visible_compression_animation_and_static_status_marker() {
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
        state.running = true;
        state.compacting = true;
        state.status = "Compacting conversation context…".into();
        state.status_tone = StatusTone::Active;
        state.apply(&AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "Attested end-to-end encrypted inference connected".into(),
        });

        state.animation_frame = 0;
        let spread = render_symbols(100, 30, &state);
        state.animation_frame = 4;
        let compressed = render_symbols(100, 30, &state);
        assert!(spread.contains("● [▰   ▰   ▰]  Compacting"));
        assert!(compressed.contains("● [    ◆    ]  Compacting"));
        assert!(!spread.contains("Attested end-to-end"));
        assert_ne!(spread, compressed);

        state.options.ascii = true;
        assert_eq!(state.compaction_frame(), "[    @    ]");
        state.apply(&AppEvent::ContextCompacted {
            summary: "successor".into(),
            messages_before: 4,
        });
        assert!(!state.compacting);
        assert!(render_symbols(100, 30, &state).contains("* Context compacted"));
    }

    #[test]
    fn provider_connection_details_do_not_replace_task_status_but_failures_do() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        let turn_id = TurnId::new();
        state.apply(&AppEvent::TurnStarted { turn_id });
        state.apply(&AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "Attested end-to-end encrypted inference connected".into(),
        });
        assert_eq!(state.status, "Working…");

        state.apply(&AppEvent::ProviderStatusChanged {
            connected: false,
            detail: "connection closed".into(),
        });
        assert_eq!(state.status, "Provider unavailable: connection closed");
        assert_eq!(state.status_tone, StatusTone::Error);
    }

    #[test]
    fn slash_setting_events_update_the_header_and_compaction_transcript() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "old-model".into(),
            PermissionProfile::Confirm,
        );
        state.apply(&AppEvent::ModelChanged {
            model: "new-model".into(),
        });
        state.apply(&AppEvent::ThinkingLevelChanged {
            level: ThinkingLevel::High,
        });
        state.apply(&AppEvent::ContextCompacted {
            summary: "hidden successor summary".into(),
            messages_before: 12,
        });
        let output = render_symbols(100, 30, &state);
        assert!(output.contains("new-model · high · Confirm"));
        assert!(output.contains("CONTEXT COMPACTED"));
        assert!(output.contains("12 conversation message(s)"));
        assert!(!output.contains("hidden successor summary"));
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
    fn startup_card_has_brand_version_site_splash_and_cherry_art() {
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
        state.splash_message = "Think freely.";
        let output = render_symbols(100, 30, &state);
        assert!(output.contains("AXIOM CLI"));
        assert!(output.contains(concat!("v", env!("CARGO_PKG_VERSION"))));
        assert!(output.contains("Think freely."));
        assert!(output.contains("⢧⣿⣴⣃⣘⡾"));
        assert!(!output.contains("AxiomCLI is ready"));

        let compact = render_symbols(64, 30, &state);
        assert!(compact.contains("⣬⣞⣺⣧⣲⣿"));
        assert!(compact.contains("Think freely."));

        let narrow = render_symbols(54, 16, &state);
        assert!(narrow.contains("✿  AXIOM CLI"));
        assert!(narrow.contains("Think freely."));
    }

    #[test]
    fn base_background_paints_axiom_black_at_every_outer_cell() {
        let state = TuiState::with_options(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
            TuiOptions {
                color_mode: ColorMode::TrueColor,
                ascii: false,
                animation: false,
            },
        );
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &state)).expect("draw");
        let buffer = terminal.backend().buffer();
        for position in [(0, 0), (99, 0), (0, 29), (99, 29)] {
            assert_eq!(buffer[position].bg, Color::Rgb(16, 16, 19));
        }
    }

    #[test]
    fn escape_stop_cancels_a_running_turn_and_updates_the_controls() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        let turn_id = TurnId::new();
        state.apply(&AppEvent::PromptAccepted {
            turn_id: turn_id.clone(),
            text: "keep working".into(),
        });
        state.apply(&AppEvent::TurnStarted {
            turn_id: turn_id.clone(),
        });
        state.overlay = Some(Overlay::Help);
        state.search_active = true;
        state.answer_prompt = Some("answer".into());
        state
            .pending_attention
            .insert("question:one".into(), "answer".into());
        let cancellation = CancellationToken::new();

        assert!(stop_running_turn(&mut state, Some(&cancellation)));
        assert!(cancellation.is_cancelled());
        assert_eq!(state.status, "Stopping task…");
        assert!(state.overlay.is_none());
        assert!(!state.search_active);
        assert!(state.answer_prompt.is_none());
        let rendered = render_symbols(100, 30, &state);
        assert!(rendered.contains("Esc stop"));
        assert!(!rendered.contains("Enter send/toggle"));

        state.apply(&AppEvent::TurnCancelled { turn_id });
        assert_eq!(state.status, "Cancelled");
        assert!(state.pending_attention.is_empty());
    }

    #[test]
    fn escape_clears_the_composer_draft_and_resets_completion() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.input = "/thinking m".into();
        state.complete_slash();
        state.focus = Focus::Transcript;

        assert!(state.clear_composer_draft());
        assert!(state.input.is_empty());
        assert!(state.slash_completion.is_none());
        assert_eq!(state.focus, Focus::Composer);
        assert_eq!(state.status, "Draft cleared");
        assert!(!state.clear_composer_draft());
    }

    #[test]
    fn token_usage_accumulates_across_provider_requests_without_rendering() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.apply(&AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "connected".into(),
        });
        state.apply(&AppEvent::UsageUpdated {
            input_tokens: 1_000,
            output_tokens: 100,
        });
        state.apply(&AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "connected".into(),
        });
        state.apply(&AppEvent::UsageUpdated {
            input_tokens: 120,
            output_tokens: 14,
        });

        assert_eq!(state.input_tokens, 1_120);
        assert_eq!(state.output_tokens, 114);
        assert_eq!(state.displayed_tokens(), 1_234);
        assert!(!render_symbols(100, 30, &state).contains("tokens used"));
    }

    #[test]
    fn token_usage_label_handles_zero_singular_and_grouping() {
        assert_eq!(format_token_usage(0), "0 tokens used");
        assert_eq!(format_token_usage(1), "1 token used");
        assert_eq!(format_token_usage(12_345_678), "12,345,678 tokens used");
        assert_eq!(format_token_usage(42), "42 tokens used");
    }

    #[test]
    fn token_counter_advances_during_streaming_then_reconciles_to_exact_usage() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        let turn_id = TurnId::new();
        state.apply(&AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "connected".into(),
        });
        state.apply(&AppEvent::TextDelta {
            turn_id: turn_id.clone(),
            text: "12345678".into(),
        });
        state.apply(&AppEvent::ReasoningDelta {
            turn_id,
            text: "1234".into(),
        });

        assert_eq!(state.displayed_tokens(), 3);
        assert!(!render_symbols(100, 30, &state).contains("tokens used"));

        state.apply(&AppEvent::UsageUpdated {
            input_tokens: 40,
            output_tokens: 5,
        });
        assert_eq!(state.displayed_tokens(), 45);
        assert!(!render_symbols(100, 30, &state).contains("tokens used"));

        state.apply(&AppEvent::TextDelta {
            turn_id: TurnId::new(),
            text: "12345678".into(),
        });
        assert_eq!(state.displayed_tokens(), 47);
        state.apply(&AppEvent::UsageUpdated {
            input_tokens: 40,
            output_tokens: 8,
        });
        assert_eq!(state.displayed_tokens(), 48);
        assert_eq!(state.input_tokens, 40);
        assert_eq!(state.output_tokens, 8);
    }

    #[test]
    fn pending_attention_is_ordered_and_does_not_replace_message_draft() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
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
                    required: true,
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
    fn optional_questions_accept_a_blank_answer_but_required_questions_do_not() {
        let optional = crate::app::QuestionSpec {
            id: "details".into(),
            prompt: "Anything else?".into(),
            options: Vec::new(),
            multiple: false,
            required: false,
        };
        assert_eq!(
            parse_tui_answer("   ", &optional).expect("skip"),
            Vec::<String>::new()
        );

        let required = crate::app::QuestionSpec {
            required: true,
            ..optional
        };
        assert!(parse_tui_answer("", &required).is_err());
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
                assert!(contents.contains("NOT VERIFIED"));
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
    fn attestation_failure_is_actionable_and_keeps_raw_detail_folded() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        let turn = TurnId::new();
        state.apply(&AppEvent::PromptAccepted {
            turn_id: turn.clone(),
            text: "test secure inference".into(),
        });
        state.apply(&AppEvent::TurnStarted {
            turn_id: turn.clone(),
        });
        let raw = "provider error: AttestationRejected: Intel TDX status is rejected by policy";
        state.apply(&AppEvent::ErrorRaised {
            turn_id: Some(turn.clone()),
            message: raw.into(),
        });

        let rendered = render_symbols(100, 30, &state);
        assert!(rendered.contains("Secure connection could not be verified"));
        assert!(rendered.contains("Your request was not sent"));
        assert!(!rendered.contains("Intel TDX"));
        assert_eq!(state.status, "Attestation failed");
        let task = state.entries[state.task_index(&turn).expect("task")]
            .task
            .as_ref()
            .expect("task view");
        assert_eq!(task.error_detail.as_deref(), Some(raw));
    }

    #[test]
    fn ascii_mode_and_every_semantic_category_render_without_controls() {
        let mut state = TuiState::with_options(
            PathBuf::from("/tmp/🌸/project"),
            "model".into(),
            PermissionProfile::Confirm,
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
                profile: PermissionProfile::Confirm,
            },
            AppEvent::SessionResumed {
                cwd: PathBuf::from("/tmp/project"),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
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
                effect: serde_json::json!({"kind":"file_write","path":"src/main.rs"}),
                choices: vec!["allow_once".into(), "deny".into()],
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
                files: Vec::new(),
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
                profile: PermissionProfile::Confirm,
            },
            AppEvent::ModelChanged {
                model: "next-model".into(),
            },
            AppEvent::ThinkingLevelChanged {
                level: ThinkingLevel::Low,
            },
            AppEvent::ContextCompacted {
                summary: "successor".into(),
                messages_before: 2,
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
    fn first_scroll_step_leaves_follow_mode_at_the_real_bottom_offset() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        for index in 0..40 {
            state.push_entry(
                EntryKind::Assistant,
                format!("scroll fixture line {index}"),
                false,
            );
        }
        let area = Rect::new(0, 0, 100, 30);
        let bottom = transcript_max_scroll(&state, area);
        assert!(bottom > 3);
        assert!(state.follow_output);
        assert_eq!(state.scroll, 0);

        scroll_transcript_up(&mut state, area, 3);
        assert!(!state.follow_output);
        assert_eq!(state.scroll, bottom - 3);

        scroll_transcript_down(&mut state, area, 3);
        assert!(state.follow_output);
        assert_eq!(state.scroll, 0);
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
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "I’ll inspect the README first.".into(),
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
                files: Vec::new(),
            },
            AppEvent::ToolCompleted {
                call_id: "read-1".into(),
                success: true,
            },
            AppEvent::TextDelta {
                turn_id: turn.clone(),
                text: "This repository is AxiomCLI.".into(),
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
        assert_eq!(task.timeline.len(), 3);

        let folded = render_symbols(100, 30, &state);
        assert!(folded.contains("Read `README.md`"));
        assert!(!folded.contains("private scratch reasoning"));
        assert!(!folded.contains("raw README contents"));
        let lines = folded.lines().collect::<Vec<_>>();
        let before = lines
            .iter()
            .position(|line| line.contains("I’ll inspect the README first."))
            .expect("response before tool");
        let tool = lines
            .iter()
            .position(|line| line.contains("Read `README.md`"))
            .expect("tool timeline row");
        let after = lines
            .iter()
            .position(|line| line.contains("This repository is AxiomCLI."))
            .expect("response after tool");
        assert!(before + 1 < tool, "tool should have spacing before it");
        assert!(tool + 1 < after, "tool should have spacing after it");

        state.selected = state.task_index(&turn);
        state.toggle_selected();
        let expanded = render_symbols(100, 40, &state);
        assert!(expanded.contains("private scratch reasoning"));
        assert!(expanded.contains("raw README contents"));
    }

    #[test]
    fn tool_feedback_shows_live_commands_queries_and_workspace_relative_paths() {
        let mut state = TuiState::new(
            PathBuf::from("/work/axiomai"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        let turn = TurnId::new();
        state.apply(&AppEvent::PromptAccepted {
            turn_id: turn.clone(),
            text: "inspect and test".into(),
        });
        state.apply(&AppEvent::TurnStarted {
            turn_id: turn.clone(),
        });

        state.apply(&AppEvent::ToolProposed {
            turn_id: turn.clone(),
            call_id: "command".into(),
            name: "run_command".into(),
            arguments: serde_json::json!({
                "program": "cargo",
                "args": ["test", "-p", "axiomcli"]
            }),
        });
        assert_eq!(state.status, "Running a command: `cargo test -p axiomcli`");
        assert!(
            render_symbols(120, 34, &state).contains("Running a command: `cargo test -p axiomcli`")
        );
        state.apply(&AppEvent::ToolCompleted {
            call_id: "command".into(),
            success: true,
        });
        assert_eq!(state.status, "Ran a command: `cargo test -p axiomcli`");

        for (call_id, name, arguments, expected) in [
            (
                "read",
                "read_file",
                serde_json::json!({"path":"/work/axiomai/src/lib.rs"}),
                "Read `src/lib.rs`",
            ),
            (
                "web",
                "web_search",
                serde_json::json!({"query":"Axiom TEE documentation"}),
                "Searched the web for `Axiom TEE documentation`",
            ),
            (
                "git",
                "run_command",
                serde_json::json!({"program":"git", "args":["diff", "--stat"]}),
                "Ran Git command: `git diff --stat`",
            ),
        ] {
            state.apply(&AppEvent::ToolProposed {
                turn_id: turn.clone(),
                call_id: call_id.into(),
                name: name.into(),
                arguments,
            });
            state.apply(&AppEvent::ToolCompleted {
                call_id: call_id.into(),
                success: true,
            });
            assert_eq!(state.status, expected);
        }

        let rendered = render_symbols(120, 42, &state);
        for expected in [
            "Ran a command: `cargo test -p axiomcli`",
            "Read `src/lib.rs`",
            "Searched the web for `Axiom TEE documentation`",
            "Ran Git command: `git diff --stat`",
        ] {
            assert!(
                rendered.contains(expected),
                "missing tool feedback: {expected}"
            );
        }
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
        state.apply(&AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "connected".into(),
        });
        state.apply(&AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: "I’ll inspect the relevant files.".into(),
        });
        state.apply(&AppEvent::UsageUpdated {
            input_tokens: 40,
            output_tokens: 7,
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
        state.apply(&AppEvent::ProviderStatusChanged {
            connected: true,
            detail: "connected".into(),
        });
        state.apply(&AppEvent::TextDelta {
            turn_id: turn.clone(),
            text: "The repository contains a Rust coding agent.".into(),
        });
        state.apply(&AppEvent::UsageUpdated {
            input_tokens: 60,
            output_tokens: 10,
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
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tui-preview");
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
        let mut startup = preview_state();
        startup.splash_message = "A quiet place to think.";
        let mut startup_compact = preview_state();
        startup_compact.splash_message = "A quiet place to think.";
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
                effect: serde_json::json!({"kind":"file_write","path":"RELEASE.md"}),
                choices: vec!["allow_once".into(), "deny".into()],
            },
        ] {
            waiting.apply(&event);
        }
        waiting.interaction = Some(InteractionView::Approval(ApprovalRequest {
            request_id: "permission-1".into(),
            explanation: "Workspace policy requires confirmation before editing RELEASE.md".into(),
            effect: crate::policy::Effect::FileWrite {
                path: PathBuf::from("/work/axiomai/RELEASE.md"),
            },
            allow_session_grants: true,
            suggested_prefix_scope: Some("FileWrite within /work/axiomai".into()),
        }));

        let mut question = preview_state();
        let question_turn = TurnId::new();
        question.apply(&AppEvent::PromptAccepted {
            turn_id: question_turn.clone(),
            text: "prepare the release notes".into(),
        });
        question.apply(&AppEvent::TurnStarted {
            turn_id: question_turn,
        });
        let question_request = QuestionRequest {
            request_id: "release-format".into(),
            questions: vec![crate::app::QuestionSpec {
                id: "audience".into(),
                prompt: "Who are these release notes for?".into(),
                options: vec!["Developers".into(), "End users".into(), "Both".into()],
                multiple: false,
                required: true,
            }],
        };
        question.apply(&AppEvent::QuestionsAsked {
            request: question_request.clone(),
        });
        question.interaction = Some(InteractionView::Questions {
            request: question_request,
            index: 0,
        });
        question.answer_prompt = Some("Question 1/1: Who are these release notes for?".into());
        question.focus = Focus::Composer;

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
                arguments: serde_json::json!({"query": "call_id", "path": "apps/axiomcli/src/tui"}),
            },
            AppEvent::ToolStarted {
                call_id: "search-reducer".into(),
                name: "search_files".into(),
            },
            AppEvent::ToolOutput {
                call_id: "search-reducer".into(),
                content: "apps/axiomcli/src/tui.rs:511\napps/axiomcli/src/tui/task.rs:142".into(),
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
                arguments: serde_json::json!({"path": "apps/axiomcli/src/tui.rs"}),
            },
            AppEvent::ToolStarted {
                call_id: "patch-reducer".into(),
                name: "apply_patch".into(),
            },
            AppEvent::DiffAvailable {
                call_id: "patch-reducer".into(),
                diff: "- push_entry(tool_output)\n+ update_tool(call_id)".into(),
                truncated: false,
                files: Vec::new(),
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

        let mut slash_commands = preview_state();
        slash_commands.input = "/".into();
        slash_commands.status = "Choose a local command".into();

        let mut slash_options = preview_state();
        slash_options.input = "/thinking h".into();
        slash_options.status = "Tab completes valid options".into();

        let mut model_picker = preview_state();
        model_picker.model_candidates = vec![
            "claude-sonnet-4-5".into(),
            "glm-5-2".into(),
            "gpt-5.2-codex".into(),
            "grok-code-fast-1".into(),
            "qwen3-coder".into(),
        ];
        model_picker.model_catalog_loaded = true;
        model_picker.open_model_picker();
        model_picker.push_model_picker_search('g');

        let mut resume_picker = preview_state();
        resume_picker.open_resume_picker(vec![
            SessionSummary {
                id: "d6bfde31-2742-45df-8744-72c08b2ce82c".into(),
                title: Some("Build transcript resume picker".into()),
                cwd: PathBuf::from("/work/axiomai"),
                origin: "tui".into(),
                profile: "full_access".into(),
                archived: false,
                history_truncated: false,
                updated_at: "2026-08-22T16:42:00Z".into(),
            },
            SessionSummary {
                id: "0b8b86dd-f972-42cf-a175-9b4bf73b0fa6".into(),
                title: Some("Repair Markdown rendering".into()),
                cwd: PathBuf::from("/work/axiomai"),
                origin: "tui".into(),
                profile: "confirm".into(),
                archived: false,
                history_truncated: false,
                updated_at: "2026-08-22T14:10:00Z".into(),
            },
            SessionSummary {
                id: "3aef7c2a-98df-4438-a18a-e158187f6f7a".into(),
                title: None,
                cwd: PathBuf::from("/work/axiomai"),
                origin: "tui".into(),
                profile: "confirm".into(),
                archived: false,
                history_truncated: false,
                updated_at: "2026-08-21T09:06:00Z".into(),
            },
        ]);

        let mut security = preview_state();
        security.security = "SECURE";
        security.security_evidence = Some(security_evidence());
        security.open_security();

        let mut security_workload = preview_state();
        security_workload.security = "SECURE";
        security_workload.security_evidence = Some(security_evidence());
        security_workload.overlay = Some(Overlay::Security(SecurityView::Workload));

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
            ("startup", 100, 30, startup),
            ("startup-compact", 64, 30, startup_compact),
            ("conversation", 100, 30, conversation),
            ("working", 100, 30, working),
            ("waiting", 100, 30, waiting),
            ("question", 100, 30, question),
            ("details", 100, 42, details),
            ("bloom-rest", 84, 20, bloom_preview(0)),
            ("bloom-open", 84, 20, bloom_preview(5)),
            ("bloom-full", 84, 20, bloom_preview(7)),
            ("bloom-wilt", 84, 20, bloom_preview(11)),
            ("browsing", 100, 30, browsing),
            ("out-of-credits", 100, 30, out_of_credits),
            ("overlay", 100, 30, overlay),
            ("slash-commands", 100, 30, slash_commands),
            ("slash-options", 100, 30, slash_options),
            ("model-picker", 100, 30, model_picker),
            ("resume-picker", 100, 30, resume_picker),
            ("security", 120, 42, security),
            ("security-workload", 120, 42, security_workload),
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
    fn single_line_fields_keep_their_caret_visible_for_overlong_input() {
        assert_eq!(tail_to_width("abcdef", 10), "abcdef");
        assert_eq!(tail_to_width("abcdef", 3), "def");
        assert_eq!(tail_to_width("a👋bc", 3), "bc");
    }

    #[test]
    fn composer_grows_to_eight_rows_then_scrolls_with_the_editing_cursor() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.has_prompt = true;
        state.entries.clear();
        let area = Rect::new(0, 0, 100, 42);
        assert_eq!(main_rows(area, &state)[3].height, 3);

        state.input.set_text(
            (0..20)
                .map(|index| format!("draft line {index:02}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let rows = main_rows(area, &state);
        assert_eq!(rows[3].height, COMPOSER_MAX_CONTENT_ROWS + 2);
        assert!(
            rows[1].height >= 3,
            "the transcript must retain usable space"
        );

        let bottom = render_symbols(area.width, area.height, &state);
        assert!(bottom.contains("draft line 19"));
        assert!(!bottom.contains("draft line 00"));

        state.input.move_to_document_start();
        let top = render_symbols(area.width, area.height, &state);
        assert!(top.contains("draft line 00"));
        assert!(!top.contains("draft line 19"));
    }

    #[test]
    fn multiline_composer_keeps_minimum_terminal_layout_usable() {
        let mut state = TuiState::new(
            PathBuf::from("/tmp/project"),
            "model".into(),
            PermissionProfile::Confirm,
        );
        state.input.set_text("one\ntwo\nthree\nfour");
        let area = Rect::new(0, 0, MIN_WIDTH, MIN_HEIGHT);
        let rows = main_rows(area, &state);
        assert_eq!(rows[1].height, 3);
        assert_eq!(rows[3].height, 3);
        assert!(render_symbols(area.width, area.height, &state).contains("four"));
    }

    #[test]
    fn command_errors_are_transient_status_instead_of_transcript_entries() {
        let mut state = preview_state();
        let entries_before = state.entries.len();
        show_slash_error(&mut state, "/permissions does not accept arguments");
        assert_eq!(state.entries.len(), entries_before);
        assert!(state.status.starts_with("Command error:"));
        assert_eq!(state.status_tone, StatusTone::Error);

        state.apply(&AppEvent::PermissionProfileChanged {
            profile: PermissionProfile::FullAccess,
        });
        assert_eq!(state.status, "Tools permission changed to Full Access");
    }

    #[test]
    fn verifying_attestation_animates_the_header_chip() {
        let mut state = preview_state();
        state.options.animation = true;
        state.apply(&AppEvent::SecurityStatusChanged {
            state: crate::app::SecurityStatus::Verifying,
        });
        state.animation_frame = 0;
        let resting = render_symbols(100, 30, &state);
        state.animation_frame = 4;
        let blooming = render_symbols(100, 30, &state);
        assert!(state.should_animate());
        assert!(resting.contains("· VERIFYING"));
        assert!(blooming.contains("✦ VERIFYING"));
    }

    #[test]
    fn auth_overlays_are_keyboard_clear_and_never_render_pasted_secrets() {
        let mut state = preview_state();
        state.open_login(Some("Authentication is required.".into()));
        let menu = render_symbols(90, 28, &state);
        assert!(menu.contains("Continue in browser"));
        assert!(menu.contains("Continue with an Axiom API key"));
        assert!(!menu.contains("Not now"));
        assert!(!menu.contains("Press K"));

        state.overlay = Some(Overlay::Auth(AuthOverlay::Paste {
            kind: AuthPasteKind::ApiKey,
            input: SecretInput("axm_do-not-render-this-secret".into()),
        }));
        let paste = render_symbols(90, 28, &state);
        assert!(paste.contains("Paste your Axiom API key"));
        assert!(paste.contains('•'));
        assert!(!paste.contains("do-not-render-this-secret"));
        assert!(!format!("{state:?}").contains("do-not-render-this-secret"));

        state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
            status: "Signed in successfully.\n\nYour Axiom API key is saved securely.".into(),
        }));
        let account = render_symbols(90, 28, &state);
        let account_lines = account.lines().collect::<Vec<_>>();
        let signed_row = account_lines
            .iter()
            .position(|line| line.contains("Signed in successfully."))
            .expect("signed-in row");
        let storage_row = account_lines
            .iter()
            .position(|line| line.contains("Your Axiom API key is saved securely."))
            .expect("storage row");
        assert!(storage_row >= signed_row + 2);
        assert!(account.contains("Your Axiom API key is saved securely."));
        assert!(!account.contains("Key ID:"));
    }

    #[test]
    fn unattested_badge_requires_the_explicit_development_launch_gate() {
        if crate::provider::unattested_development_enabled() {
            return;
        }
        let mut state = preview_state();
        state.apply(&AppEvent::SecurityStatusChanged {
            state: crate::app::SecurityStatus::UnattestedDevelopment,
        });
        assert_eq!(state.security, "SECURITY FAILED");
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
