//! UI state, local transitions, and application-event reduction.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    env,
    path::PathBuf,
    str::FromStr as _,
    time::Instant,
};

use axiom_secure_client::SecurityEvidence;

use crate::{
    AxiomError, Result,
    app::{AppEvent, PermissionProfile, QuestionRequest, SessionId, ThinkingLevel, TurnId},
    auth::AuthManager,
    policy::ApprovalRequest,
    session::{SessionStore, SessionSummary},
    slash::{self},
};

use super::{
    attestation::{
        ReportStatus, attestation_export_filename, attestation_picker_directories, report_status,
        write_attestation_export,
    },
    composer::ComposerBuffer,
    markdown::StreamingMarkdownRenderer,
    pick_splash_message,
    proof_refresh::ProofRefresh,
    task::{TaskActivity, TaskPhase, TaskRunView, ToolPhase},
    text::{is_attestation_failure, sanitize_terminal_text, user_facing_provider_error},
    theme::{Appearance, ColorMode, Theme},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Focus {
    None,
    Composer,
    Transcript,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Overlay {
    Billing(Box<super::billing::BillingView>),
    Entry(usize),
    Help,
    Usage,
    Permissions {
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
pub(super) enum SavePickerFocus {
    Directories,
    #[default]
    Filename,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SaveAttestationPicker {
    pub(super) directory: PathBuf,
    pub(super) filename: String,
    pub(super) selected: usize,
    pub(super) focus: SavePickerFocus,
    pub(super) error: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SecurityView {
    #[default]
    Summary,
    Workload,
    Raw,
}

impl SecurityView {
    pub(super) const fn next(self) -> Self {
        match self {
            Self::Summary => Self::Workload,
            Self::Workload => Self::Raw,
            Self::Raw => Self::Summary,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum AuthOverlay {
    Menu {
        message: Option<String>,
    },
    Starting,
    Browser {
        user_code: String,
        authorization_url: String,
        browser_opened: bool,
    },
    Checking {
        label: &'static str,
    },
    Account {
        status: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StatusTone {
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
    pub appearance: Appearance,
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
    pub(super) fn theme(self) -> Theme {
        Theme::for_appearance(self.color_mode, self.appearance)
    }

    #[must_use]
    pub fn from_environment(animation: bool) -> Self {
        let ascii = env::var("AXIOMCLI_ASCII").as_deref() == Ok("1")
            || env::var("TERM").as_deref() == Ok("dumb");
        Self {
            color_mode: ColorMode::detect(),
            appearance: Appearance::from_environment(),
            ascii,
            animation: animation && env::var("AXIOMCLI_REDUCED_MOTION").as_deref() != Ok("1"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntryKind {
    User,
    Assistant,
    Task,
    Reasoning,
    Tool,
    System,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EntryFormat {
    Plain,
    Markdown,
}

#[derive(Clone, Debug)]
pub(super) struct Entry {
    pub(super) id: u64,
    pub(super) kind: EntryKind,
    pub(super) format: EntryFormat,
    pub(super) text: String,
    pub(super) collapsed: bool,
    pub(super) markdown: RefCell<StreamingMarkdownRenderer>,
    pub(super) task: Option<TaskRunView>,
}

#[derive(Debug)]
pub(super) struct SlashCompletionCycle {
    pub(super) candidates: Vec<slash::SlashCompletion>,
    pub(super) index: usize,
}

#[derive(Clone, Debug)]
pub(super) enum InteractionView {
    Approval(ApprovalRequest),
    Questions {
        request: QuestionRequest,
        index: usize,
    },
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct TuiState {
    pub(super) cwd: PathBuf,
    pub(super) model: String,
    pub(super) thinking: ThinkingLevel,
    pub(super) splash_message: &'static str,
    pub(super) model_candidates: Vec<String>,
    pub(super) model_details: BTreeMap<String, axiom_inference::ModelInfo>,
    pub(super) model_catalog_loaded: bool,
    pub(super) model_catalog_loading: bool,
    pub(super) slash_completion: Option<SlashCompletionCycle>,
    pub(super) slash_selection: usize,
    pub(super) slash_selection_moved: bool,
    pub(super) profile: PermissionProfile,
    pub(super) web_enabled: bool,
    pub(super) security: &'static str,
    pub(super) security_evidence: Option<SecurityEvidence>,
    pub(super) security_error: Option<String>,
    pub(super) proof_refresh: ProofRefresh,
    pub(super) input: ComposerBuffer,
    pub(super) entries: Vec<Entry>,
    pub(super) running: bool,
    pub(super) status: String,
    pub(super) status_tone: StatusTone,
    pub(super) scroll: u16,
    pub(super) follow_output: bool,
    pub(super) focus: Focus,
    pub(super) selected: Option<usize>,
    pub(super) search_active: bool,
    pub(super) search_query: String,
    pub(super) search_matches: Vec<usize>,
    pub(super) search_match: usize,
    pub(super) overlay: Option<Overlay>,
    pub(super) overlay_scroll: u16,
    pub(super) exit_armed: bool,
    pub(super) options: TuiOptions,
    pub(super) next_entry_id: u64,
    pub(super) pending_attention: BTreeMap<String, String>,
    pub(super) answer_input: String,
    pub(super) answer_prompt: Option<String>,
    pub(super) interaction: Option<InteractionView>,
    pub(super) request_usage: BTreeMap<String, axiom_inference::RequestUsage>,
    pub(super) context_usage: Option<axiom_acp_extension::ContextUsage>,
    pub(super) animation_frame: u64,
    pub(super) transition_ticks: u8,
    pub(super) compacting: bool,
    pub(super) has_prompt: bool,
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
            model_details: BTreeMap::new(),
            model_catalog_loaded: false,
            model_catalog_loading: false,
            slash_completion: None,
            slash_selection: 0,
            slash_selection_moved: false,
            profile,
            web_enabled: false,
            security: "NOT VERIFIED",
            security_evidence: None,
            security_error: None,
            proof_refresh: ProofRefresh::new(Instant::now()),
            input: ComposerBuffer::default(),
            entries: Vec::new(),
            running: false,
            status: "Ready".into(),
            status_tone: StatusTone::Neutral,
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
            next_entry_id: 1,
            pending_attention: BTreeMap::new(),
            answer_input: String::new(),
            answer_prompt: None,
            interaction: None,
            request_usage: BTreeMap::new(),
            context_usage: None,
            animation_frame: 0,
            transition_ticks: 0,
            compacting: false,
            has_prompt: false,
        }
    }

    pub(super) fn open_login(&mut self, message: Option<String>) {
        self.overlay = Some(Overlay::Auth(AuthOverlay::Menu { message }));
        self.overlay_scroll = 0;
        self.status = "Sign in to Axiom".into();
        self.status_tone = StatusTone::Waiting;
    }

    pub(super) fn reset_security(&mut self) {
        self.security = "NOT VERIFIED";
        self.security_evidence = None;
        self.security_error = None;
        self.proof_refresh = ProofRefresh::new(Instant::now());
    }

    pub(super) fn fail_security(&mut self, detail: &str) {
        self.security = "SECURITY FAILED";
        self.security_evidence = None;
        self.security_error = Some(sanitize_terminal_text(detail));
    }

    pub(super) fn report_status(&self) -> ReportStatus {
        if !self.running && !self.compacting && self.proof_refresh.is_idle(Instant::now()) {
            return ReportStatus::Idle;
        }
        report_status(
            self.security,
            &self.model,
            self.security_evidence.as_ref(),
            u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0),
        )
    }

    pub(super) fn security_refresh_due(&self, now: Instant, unix_seconds: u64) -> bool {
        if self.running
            || self.compacting
            || matches!(self.security, "VERIFYING" | "TEE OUTDATED")
            || !self.proof_refresh.due(now)
        {
            return false;
        }
        let report = report_status(
            self.security,
            &self.model,
            self.security_evidence.as_ref(),
            unix_seconds,
        );
        !matches!(report, ReportStatus::Verified | ReportStatus::Degraded)
    }

    pub(super) fn open_security(&mut self) {
        self.overlay = Some(Overlay::Security(SecurityView::Summary));
        self.overlay_scroll = 0;
    }

    pub(super) fn cycle_security_view(&mut self) {
        if let Some(Overlay::Security(view)) = &mut self.overlay {
            *view = view.next();
            self.overlay_scroll = 0;
        }
    }

    pub(super) fn open_attestation_save_picker(&mut self) {
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

    pub(super) fn close_attestation_save_picker(&mut self) {
        self.overlay = Some(Overlay::Security(SecurityView::Raw));
        self.overlay_scroll = 0;
    }

    pub(super) fn toggle_attestation_save_focus(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        picker.focus = match picker.focus {
            SavePickerFocus::Directories => SavePickerFocus::Filename,
            SavePickerFocus::Filename => SavePickerFocus::Directories,
        };
        picker.error = None;
    }

    pub(super) fn move_attestation_directory_selection(&mut self, amount: isize) {
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

    pub(super) fn open_selected_attestation_directory(&mut self) {
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

    pub(super) fn open_parent_attestation_directory(&mut self) {
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

    pub(super) fn push_attestation_filename(&mut self, text: &str) {
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

    pub(super) fn pop_attestation_filename(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        if picker.focus == SavePickerFocus::Filename {
            picker.filename.pop();
            picker.error = None;
        }
    }

    pub(super) fn clear_attestation_filename(&mut self) {
        let Some(Overlay::SaveAttestation(picker)) = &mut self.overlay else {
            return;
        };
        if picker.focus == SavePickerFocus::Filename {
            picker.filename.clear();
            picker.error = None;
        }
    }

    pub(super) fn save_attestation_export(&mut self) {
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

    pub(super) fn push_entry(&mut self, kind: EntryKind, text: String, collapsed: bool) {
        let format = if kind == EntryKind::Assistant {
            EntryFormat::Markdown
        } else {
            EntryFormat::Plain
        };
        self.push_entry_with_format(kind, format, text, collapsed);
    }

    pub(super) fn push_entry_with_format(
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

    pub(super) fn push_task(&mut self, turn_id: TurnId, prompt: String) -> usize {
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

    pub(super) fn task_index(&self, turn_id: &TurnId) -> Option<usize> {
        self.entries.iter().rposition(|entry| {
            entry
                .task
                .as_ref()
                .is_some_and(|task| task.turn_id == *turn_id)
        })
    }

    pub(super) fn ensure_task(&mut self, turn_id: &TurnId) -> usize {
        self.task_index(turn_id)
            .unwrap_or_else(|| self.push_task(turn_id.clone(), "Task".into()))
    }

    pub(super) fn active_task_mut(&mut self) -> Option<&mut TaskRunView> {
        self.entries
            .iter_mut()
            .rev()
            .filter_map(|entry| entry.task.as_mut())
            .find(|task| !task.phase.is_terminal())
    }

    pub(super) fn task_for_call_mut(&mut self, call_id: &str) -> Option<&mut TaskRunView> {
        self.entries
            .iter_mut()
            .rev()
            .filter_map(|entry| entry.task.as_mut())
            .find(|task| task.tools.iter().any(|tool| tool.call_id == call_id))
    }

    pub(super) fn resume_active_task(&mut self) {
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

    pub(super) fn wait_active_task(&mut self, message: String) {
        if let Some(task) = self.active_task_mut() {
            task.phase = TaskPhase::Waiting;
            task.activity = TaskActivity::Waiting(message);
        }
    }

    pub(super) fn has_pending_user_attention(&self) -> bool {
        self.pending_attention.keys().any(|key| {
            key.starts_with("permission:")
                || key.starts_with("question:")
                || key.starts_with("plan:")
        })
    }

    pub(super) fn has_background_activity(&self) -> bool {
        self.model_catalog_loading
            || self
                .pending_attention
                .keys()
                .any(|key| key.starts_with("background:"))
    }

    pub(super) fn clear_foreground_attention(&mut self) {
        self.pending_attention
            .retain(|key, _| key.starts_with("background:"));
        self.answer_input.clear();
        self.answer_prompt = None;
        self.interaction = None;
    }

    pub(super) fn clear_permission_interaction(&mut self, request_id: &str) {
        if matches!(
            &self.interaction,
            Some(InteractionView::Approval(request)) if request.request_id == request_id
        ) {
            self.interaction = None;
        }
    }

    pub(super) fn clear_question_interaction(&mut self, request_id: &str) {
        if matches!(
            &self.interaction,
            Some(InteractionView::Questions { request, .. }) if request.request_id == request_id
        ) {
            self.answer_input.clear();
            self.answer_prompt = None;
            self.interaction = None;
        }
    }

    pub(super) fn apply(&mut self, event: &AppEvent) {
        match event {
            AppEvent::SteeringApplied { turn_id, text, .. } => {
                if let Some(index) = self.task_index(turn_id)
                    && let Some(task) = self.entries[index].task.as_mut()
                {
                    task.finish(TaskPhase::Completed, None);
                }
                self.push_entry(EntryKind::User, sanitize_terminal_text(text), false);
                self.push_task(turn_id.clone(), sanitize_terminal_text(text));
                self.status = "Steering applied · Working…".into();
            }
            AppEvent::PromptAccepted { turn_id, text, .. } => {
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
            AppEvent::ContextUsageUpdated { usage } => {
                self.context_usage = Some(usage.clone());
            }
            AppEvent::RequestUsageUpdated { usage } => {
                self.request_usage
                    .insert(usage.request_id.clone(), usage.clone());
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
                self.status = format!(
                    "Permission required: {}",
                    explanation.split_whitespace().collect::<Vec<_>>().join(" ")
                );
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
                if !connected {
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
                let changed = self.model != *model;
                self.model.clone_from(model);
                if changed {
                    self.reset_security();
                    self.reset_slash_completion();
                }
                self.status = format!("Model changed to {model}");
            }
            AppEvent::ThinkingLevelChanged { level } => {
                self.thinking = *level;
                self.status = format!("Thinking set to {level}");
            }
            AppEvent::ContextCompacted {
                messages_before, ..
            } => {
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
                self.security_error = None;
                self.security = match state {
                    crate::app::SecurityStatus::Unverified => "NOT VERIFIED",
                    crate::app::SecurityStatus::Verifying => "VERIFYING",
                    crate::app::SecurityStatus::Verified => "SECURE",
                    crate::app::SecurityStatus::Degraded => "TEE WARNING",
                    crate::app::SecurityStatus::Outdated => "TEE OUTDATED",
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
                if self.security == "VERIFYING" {
                    self.reset_security();
                }
                let cancelled_turn = turn_id.to_string();
                for usage in self.request_usage.values_mut().filter(|usage| {
                    usage.turn_id.as_deref() == Some(cancelled_turn.as_str())
                        && usage.state == axiom_inference::InvocationState::Running
                }) {
                    usage.state = axiom_inference::InvocationState::Cancelled;
                    usage.error_code = Some("CANCELLED".into());
                    usage.finished_at_ms = Some(chrono::Utc::now().timestamp_millis().to_string());
                }
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
                if self.security == "SECURITY FAILED" {
                    self.security_error = Some(detail.clone());
                }
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

    pub(super) fn append_stream(&mut self, kind: EntryKind, text: &str) {
        let text = sanitize_terminal_text(text);
        if let Some(last) = self.entries.last_mut()
            && last.kind == kind
        {
            last.text.push_str(&text);
        } else {
            self.push_entry(kind, text, false);
        }
    }

    pub(super) fn should_animate(&self) -> bool {
        self.options.animation
            && (self.running
                || matches!(self.security, "VERIFYING" | "TEE OUTDATED")
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

    pub(super) fn show_startup(&self) -> bool {
        !self.has_prompt && !self.running && self.entries.is_empty()
    }

    pub(super) fn tick_animation(&mut self) {
        self.animation_frame = self.animation_frame.wrapping_add(1);
        self.transition_ticks = self.transition_ticks.saturating_sub(1);
    }

    pub(super) fn glyph<'a>(&self, unicode: &'a str, ascii: &'a str) -> &'a str {
        if self.options.ascii { ascii } else { unicode }
    }

    pub(super) fn refresh_search(&mut self) {
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

    pub(super) fn select_search_match(&mut self) {
        if let Some(index) = self.search_matches.get(self.search_match).copied() {
            self.selected = Some(index);
            self.focus = Focus::Transcript;
            self.follow_output = false;
            self.scroll = u16::try_from(index.saturating_mul(3)).unwrap_or(u16::MAX);
        }
    }

    pub(super) fn next_match(&mut self, backwards: bool) {
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

    pub(super) fn move_selection(&mut self, down: bool) {
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

    pub(super) fn toggle_selected(&mut self) {
        if let Some(entry) = self.selected.and_then(|index| self.entries.get_mut(index)) {
            entry.collapsed = !entry.collapsed;
        }
    }

    pub(super) fn reset_slash_completion(&mut self) {
        self.slash_completion = None;
        self.slash_selection = 0;
        self.slash_selection_moved = false;
    }

    pub(super) fn begin_slash_input(&mut self) {
        self.focus = Focus::Composer;
        self.reset_slash_completion();
        self.input.insert_char('/');
    }

    pub(super) fn invalid_slash_has_completion(&self) -> bool {
        self.input.starts_with('/')
            && slash::parse(&self.input).is_some_and(|command| command.is_err())
            && !self.available_slash_completions().is_empty()
    }

    fn available_slash_completions(&self) -> Vec<slash::SlashCompletion> {
        let mut candidates = slash::completions(&self.input);
        if self
            .input
            .split_once(char::is_whitespace)
            .is_some_and(|(command, _)| matches!(command, "/thinking" | "/reasoning"))
            && let Some(model) = self.model_details.get(&self.model)
        {
            let supported = crate::agent::supported_thinking_levels(model);
            candidates.retain(|candidate| {
                candidate
                    .label
                    .parse::<ThinkingLevel>()
                    .is_ok_and(|level| supported.contains(&level))
            });
        }
        candidates
    }

    pub(super) fn slash_candidates(&self) -> Vec<slash::SlashCompletion> {
        if let Some(cycle) = &self.slash_completion
            && cycle
                .candidates
                .get(cycle.index)
                .is_some_and(|candidate| candidate.input == self.input.as_str())
        {
            return cycle.candidates.clone();
        }
        self.available_slash_completions()
    }

    pub(super) fn can_navigate_slash(&self) -> bool {
        self.focus == Focus::Composer
            && self.input.starts_with('/')
            && self.input.is_cursor_at_end()
            && !self.slash_candidates().is_empty()
    }

    pub(super) fn move_slash_selection(&mut self, offset: isize) {
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

    pub(super) fn should_accept_slash_selection(&self) -> bool {
        let candidates = self.slash_candidates();
        let selected = candidates.get(self.slash_selection.min(candidates.len().saturating_sub(1)));
        selected.is_some_and(|candidate| {
            candidate.input != self.input.as_str()
                && (self.slash_selection_moved || self.invalid_slash_has_completion())
        })
    }

    pub(super) fn clear_composer_draft(&mut self) -> bool {
        if self.input.is_empty() {
            return false;
        }
        self.input.clear();
        self.reset_slash_completion();
        self.focus = Focus::Composer;
        self.status = "Draft cleared".into();
        true
    }

    pub(super) fn open_permissions(&mut self) {
        let selected = PermissionProfile::ALL
            .iter()
            .position(|profile| *profile == self.profile)
            .unwrap_or(0);
        self.overlay = Some(Overlay::Permissions { selected });
        self.status = "Choose tool permissions".into();
        self.status_tone = StatusTone::Neutral;
    }

    pub(super) fn move_permission_selection(&mut self, amount: isize) {
        let Some(Overlay::Permissions { selected }) = &mut self.overlay else {
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

    pub(super) fn selected_permission_profile(&self) -> Option<PermissionProfile> {
        let Overlay::Permissions { selected } = self.overlay.as_ref()? else {
            return None;
        };
        PermissionProfile::ALL.get(*selected).copied()
    }

    pub(super) fn open_model_picker(&mut self) {
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

    pub(super) fn model_picker_match_indices(&self) -> Vec<usize> {
        let Some(Overlay::ModelPicker { query, .. }) = &self.overlay else {
            return Vec::new();
        };
        let query = query.to_lowercase();
        self.model_candidates
            .iter()
            .enumerate()
            .filter_map(|(index, model)| {
                let matches = self.model_details.get(model).map_or_else(
                    || model.to_lowercase().contains(&query),
                    |details| {
                        [
                            details.id.as_str(),
                            details.label.as_str(),
                            details.short_label.as_str(),
                            details.upstream_model.as_str(),
                            details.provider_label.as_str(),
                        ]
                        .iter()
                        .any(|value| value.to_lowercase().contains(&query))
                    },
                );
                (query.is_empty() || matches).then_some(index)
            })
            .collect()
    }

    pub(super) fn align_model_picker_to_current(&mut self) {
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

    pub(super) fn move_model_picker(&mut self, amount: isize) {
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

    pub(super) fn push_model_picker_search(&mut self, character: char) {
        if let Some(Overlay::ModelPicker { query, selected }) = &mut self.overlay {
            query.push(character);
            *selected = 0;
        }
    }

    pub(super) fn pop_model_picker_search(&mut self) {
        if let Some(Overlay::ModelPicker { query, selected }) = &mut self.overlay {
            query.pop();
            *selected = 0;
        }
    }

    pub(super) fn selected_model(&self) -> Option<String> {
        let Overlay::ModelPicker { selected, .. } = self.overlay.as_ref()? else {
            return None;
        };
        let model_index = *self.model_picker_match_indices().get(*selected)?;
        self.model_candidates.get(model_index).cloned()
    }

    pub(super) fn open_resume_picker(&mut self, sessions: Vec<SessionSummary>) {
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

    pub(super) fn resume_picker_match_indices(&self) -> Vec<usize> {
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

    pub(super) fn move_resume_picker(&mut self, amount: isize) {
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

    pub(super) fn push_resume_picker_search(&mut self, character: char) {
        if let Some(Overlay::ResumePicker {
            query, selected, ..
        }) = &mut self.overlay
        {
            query.push(character);
            *selected = 0;
        }
    }

    pub(super) fn pop_resume_picker_search(&mut self) {
        if let Some(Overlay::ResumePicker {
            query, selected, ..
        }) = &mut self.overlay
        {
            query.pop();
            *selected = 0;
        }
    }

    pub(super) fn selected_resume_session(&self) -> Option<SessionSummary> {
        let Overlay::ResumePicker {
            selected, sessions, ..
        } = self.overlay.as_ref()?
        else {
            return None;
        };
        let session_index = *self.resume_picker_match_indices().get(*selected)?;
        sessions.get(session_index).cloned()
    }

    pub(super) fn open_delete_picker(&mut self, sessions: Vec<SessionSummary>) {
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

    pub(super) fn delete_picker_match_indices(&self) -> Vec<usize> {
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

    pub(super) fn move_delete_picker(&mut self, amount: isize) {
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

    pub(super) fn push_delete_picker_search(&mut self, character: char) {
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

    pub(super) fn pop_delete_picker_search(&mut self) {
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

    pub(super) fn toggle_delete_picker_session(&mut self) {
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

    pub(super) fn toggle_all_delete_picker_matches(&mut self) {
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

    pub(super) fn begin_delete_confirmation(&mut self) {
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

    pub(super) fn marked_delete_session_ids(&self) -> Result<Vec<SessionId>> {
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

    pub(super) fn complete_slash(&mut self) {
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

        let candidates = self.available_slash_completions();
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

    pub(super) fn accept_slash_selection(&mut self) {
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
