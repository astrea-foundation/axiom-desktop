use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    future::Future,
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicU64, Ordering},
    },
};

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{AxiomError, Result};

/// Maximum number of journal events projected before cooperative async
/// restore returns control to Tokio.
const RESTORE_COOPERATION_INTERVAL: usize = 128;

macro_rules! typed_uuid {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
                Uuid::parse_str(input).map(Self)
            }
        }
    };
}

typed_uuid!(SessionId);
typed_uuid!(TurnId);
typed_uuid!(CorrelationId);

/// Injectable runtime entropy and time. Production uses OS randomness and UTC;
/// deterministic tests use [`DeterministicServices`].
pub trait RuntimeServices: Send + Sync {
    fn next_uuid(&self) -> Uuid;
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default)]
pub struct SystemServices;

impl RuntimeServices for SystemServices {
    fn next_uuid(&self) -> Uuid {
        Uuid::new_v4()
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Stable source for golden traces. UUIDs are derived from a monotonically
/// increasing counter and timestamps advance one millisecond per event.
#[derive(Debug)]
pub struct DeterministicServices {
    counter: AtomicU64,
    epoch: DateTime<Utc>,
    clock: StdMutex<u64>,
}

impl DeterministicServices {
    #[must_use]
    pub fn new(epoch: DateTime<Utc>) -> Self {
        Self {
            counter: AtomicU64::new(1),
            epoch,
            clock: StdMutex::new(0),
        }
    }
}

impl RuntimeServices for DeterministicServices {
    fn next_uuid(&self) -> Uuid {
        let value = self.counter.fetch_add(1, Ordering::Relaxed);
        Uuid::from_u128(u128::from(value))
    }

    fn now(&self) -> DateTime<Utc> {
        let mut tick = self
            .clock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current =
            self.epoch + TimeDelta::milliseconds(i64::try_from(*tick).unwrap_or(i64::MAX));
        *tick = tick.saturating_add(1);
        current
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Tui,
    Acp,
    Headless,
    #[default]
    Test,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfile {
    None,
    Web,
    Observe,
    #[default]
    Confirm,
    FullAccess,
}

impl PermissionProfile {
    #[must_use]
    pub fn for_desktop_agent(settings: &axiom_acp_extension::DesktopAgentSettings) -> Self {
        if !settings.enabled {
            return Self::Web;
        }
        match settings.permission {
            axiom_acp_extension::DesktopAgentPermission::ApproveCommands => Self::Confirm,
            axiom_acp_extension::DesktopAgentPermission::FullAccess => Self::FullAccess,
        }
    }
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Web,
        Self::Observe,
        Self::Confirm,
        Self::FullAccess,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Web => "Web",
            Self::Observe => "Observe",
            Self::Confirm => "Confirm",
            Self::FullAccess => "Full Access",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::None => "Chat without giving the model any tools",
            Self::Web => "Allow only web search and page fetching",
            Self::Observe => "Allow read-only workspace inspection",
            Self::Confirm => "Allow every tool after one confirmation per use",
            Self::FullAccess => "Run every available tool without confirmation",
        }
    }
}

/// Provider-neutral reasoning preference for a session. Reasoning is always
/// explicit. The direct secure Axiom transport validates the selected value
/// against model catalog capabilities before sending it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    ProviderDefault,
    Enabled,
    Disabled,
    Minimal,
    Low,
    #[default]
    Medium,
    High,
    ExtraHigh,
}

impl fmt::Display for ThinkingLevel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ProviderDefault => "provider_default",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::ExtraHigh => "xhigh",
        })
    }
}

impl FromStr for ThinkingLevel {
    type Err = AxiomError;

    fn from_str(input: &str) -> Result<Self> {
        match input {
            "provider_default" => Ok(Self::ProviderDefault),
            "enabled" | "on" => Ok(Self::Enabled),
            "disabled" | "off" => Ok(Self::Disabled),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::ExtraHigh),
            _ => Err(AxiomError::Config(format!(
                "unknown thinking level `{input}`"
            ))),
        }
    }
}

impl fmt::Display for PermissionProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
            Self::Web => "web",
            Self::Observe => "observe",
            Self::Confirm => "confirm",
            Self::FullAccess => "full_access",
        })
    }
}

impl FromStr for PermissionProfile {
    type Err = AxiomError;

    fn from_str(input: &str) -> Result<Self> {
        match input {
            "none" => Ok(Self::None),
            "web" => Ok(Self::Web),
            "observe" => Ok(Self::Observe),
            "confirm" => Ok(Self::Confirm),
            "full_access" => Ok(Self::FullAccess),
            _ => Err(AxiomError::Config(format!(
                "unknown permission profile `{input}`"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycle {
    Ready,
    Running,
    WaitingForApproval,
    WaitingForAnswer,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AppCommand {
    CreateSession {
        session_id: SessionId,
        cwd: PathBuf,
        origin: Origin,
        profile: PermissionProfile,
    },
    ResumeSession {
        session_id: SessionId,
        cwd: PathBuf,
        origin: Origin,
        profile: PermissionProfile,
    },
    SubmitPrompt {
        session_id: SessionId,
        turn_id: TurnId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<axiom_inference::PromptAttachment>,
    },
    CancelTurn {
        session_id: SessionId,
        turn_id: TurnId,
    },
    FinishTurn {
        session_id: SessionId,
        turn_id: TurnId,
    },
    FailTurn {
        session_id: SessionId,
        turn_id: TurnId,
        message: String,
    },
    ChangePermissionProfile {
        session_id: SessionId,
        profile: PermissionProfile,
    },
    ConfigureDesktopAgent {
        session_id: SessionId,
        settings: axiom_acp_extension::DesktopAgentSettings,
    },
    ChangeModel {
        session_id: SessionId,
        model: String,
    },
    ChangeModelSettings {
        session_id: SessionId,
        model: String,
        thinking: ThinkingLevel,
        reset_security: bool,
    },
    ChangeThinkingLevel {
        session_id: SessionId,
        level: ThinkingLevel,
    },
    RecordContextCompaction {
        session_id: SessionId,
        summary: String,
        messages_before: usize,
    },
    CloseSession {
        session_id: SessionId,
    },
}

impl AppCommand {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::CreateSession { session_id, .. }
            | Self::ResumeSession { session_id, .. }
            | Self::SubmitPrompt { session_id, .. }
            | Self::CancelTurn { session_id, .. }
            | Self::FinishTurn { session_id, .. }
            | Self::FailTurn { session_id, .. }
            | Self::ChangePermissionProfile { session_id, .. }
            | Self::ConfigureDesktopAgent { session_id, .. }
            | Self::ChangeModel { session_id, .. }
            | Self::ChangeModelSettings { session_id, .. }
            | Self::ChangeThinkingLevel { session_id, .. }
            | Self::RecordContextCompaction { session_id, .. }
            | Self::CloseSession { session_id } => session_id,
        }
    }
}

/// Non-persisted delivery metadata. Every command reaches the kernel with a
/// correlation ID, origin and cancellation context.
#[derive(Clone, Debug)]
pub struct CommandEnvelope {
    pub correlation_id: CorrelationId,
    pub origin: Origin,
    pub cancellation: CancellationToken,
    pub command: AppCommand,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AppEvent {
    SteeringApplied {
        turn_id: TurnId,
        client_item_id: String,
        text: String,
    },
    SessionCreated {
        cwd: PathBuf,
        origin: Origin,
        profile: PermissionProfile,
    },
    SessionResumed {
        cwd: PathBuf,
        origin: Origin,
        profile: PermissionProfile,
    },
    PromptAccepted {
        turn_id: TurnId,
        text: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<axiom_inference::PromptAttachment>,
    },
    TurnStarted {
        turn_id: TurnId,
    },
    TextDelta {
        turn_id: TurnId,
        text: String,
    },
    ReasoningDelta {
        turn_id: TurnId,
        text: String,
    },
    UsageUpdated {
        input_tokens: u64,
        output_tokens: u64,
    },
    ContextUsageUpdated {
        usage: axiom_acp_extension::ContextUsage,
    },
    RequestUsageUpdated {
        usage: axiom_inference::RequestUsage,
    },
    ProgressUpdated {
        message: String,
        completed: Option<u64>,
        total: Option<u64>,
    },
    TaskListUpdated {
        items: Vec<TaskItem>,
    },
    ToolProposed {
        turn_id: TurnId,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    PermissionRequired {
        request_id: String,
        explanation: String,
        /// Serialized normalized policy effect. Kept provider-neutral so the
        /// application runtime does not depend on presentation strings.
        effect: serde_json::Value,
        /// Stable response choices that were actually offered for this request.
        choices: Vec<String>,
    },
    PermissionResolved {
        request_id: String,
        allowed: bool,
        choice: Option<String>,
    },
    ToolStarted {
        call_id: String,
        name: String,
    },
    ToolOutput {
        call_id: String,
        content: String,
        truncated: bool,
    },
    ToolCompleted {
        call_id: String,
        success: bool,
    },
    WorkspaceChanged {
        paths: Vec<PathBuf>,
    },
    DiffAvailable {
        call_id: String,
        diff: String,
        truncated: bool,
        files: Vec<FileDiff>,
    },
    QuestionAsked {
        question_id: String,
        prompt: String,
        options: Vec<String>,
    },
    QuestionAnswered {
        question_id: String,
        answers: Vec<String>,
    },
    QuestionsAsked {
        request: QuestionRequest,
    },
    QuestionsAnswered {
        request_id: String,
        answers: BTreeMap<String, Vec<String>>,
    },
    QuestionsFailed {
        request_id: String,
        reason: String,
    },
    PlanProposed {
        plan_id: String,
        revision: u64,
        markdown: String,
    },
    PlanReviewed {
        plan_id: String,
        revision: u64,
        decision: String,
    },
    BackgroundTaskChanged {
        task_id: String,
        state: String,
    },
    PermissionProfileChanged {
        profile: PermissionProfile,
    },
    DesktopAgentConfigured {
        settings: axiom_acp_extension::DesktopAgentSettings,
    },
    ModelChanged {
        model: String,
    },
    ThinkingLevelChanged {
        level: ThinkingLevel,
    },
    ContextCompacted {
        summary: String,
        messages_before: usize,
    },
    ProviderStatusChanged {
        connected: bool,
        detail: String,
    },
    SecurityStatusChanged {
        state: SecurityStatus,
    },
    /// Every provider response contributing to this completed turn carried
    /// terminal cryptographic proof. Preflight attestation is represented by
    /// `SecurityStatusChanged` and is intentionally insufficient here.
    ResponseVerified {
        turn_id: TurnId,
    },
    WarningRaised {
        message: String,
    },
    TurnCompleted {
        turn_id: TurnId,
    },
    TurnCancelled {
        turn_id: TurnId,
    },
    ErrorRaised {
        turn_id: Option<TurnId>,
        message: String,
    },
    SessionClosed,
}

/// Presentation-neutral before/after file content for clients that support
/// native diff rendering. The unified diff remains alongside this structure
/// for the TUI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    pub path: PathBuf,
    pub old_text: Option<String>,
    pub new_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskItem {
    pub id: String,
    pub title: String,
    pub status: TaskStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionSpec {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default = "question_required")]
    pub required: bool,
}

const fn question_required() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub request_id: String,
    pub questions: Vec<QuestionSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityStatus {
    Unverified,
    Verifying,
    UnattestedDevelopment,
    Verified,
    Degraded,
    Outdated,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub sequence: u64,
    pub occurred_at: DateTime<Utc>,
    pub correlation_id: CorrelationId,
    pub origin: Origin,
    pub session_id: SessionId,
    pub event: AppEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionState {
    pub id: SessionId,
    pub cwd: PathBuf,
    pub origin: Origin,
    pub profile: PermissionProfile,
    pub lifecycle: SessionLifecycle,
    pub active_turn: Option<TurnId>,
    terminal_turns: HashSet<TurnId>,
    pub pending_permission: Option<String>,
    pub pending_question: Option<String>,
}

pub struct Kernel {
    sessions: HashMap<SessionId, SessionState>,
    next_sequence: u64,
    services: Arc<dyn RuntimeServices>,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::with_services(Arc::new(SystemServices))
    }
}

impl Kernel {
    #[must_use]
    pub fn with_services(services: Arc<dyn RuntimeServices>) -> Self {
        Self {
            sessions: HashMap::new(),
            next_sequence: 0,
            services,
        }
    }

    pub fn apply(&mut self, request: CommandEnvelope) -> Result<Vec<EventEnvelope>> {
        if request.cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        let correlation_id = request.correlation_id;
        let origin = request.origin;
        let session_id = request.command.session_id().clone();
        let events = match request.command {
            AppCommand::CreateSession {
                session_id,
                cwd,
                origin,
                profile,
            } => {
                if self.sessions.contains_key(&session_id) {
                    return Err(AxiomError::InvalidTransition(format!(
                        "session {session_id} already exists"
                    )));
                }
                self.sessions.insert(
                    session_id.clone(),
                    SessionState {
                        id: session_id,
                        cwd: cwd.clone(),
                        origin,
                        profile,
                        lifecycle: SessionLifecycle::Ready,
                        active_turn: None,
                        terminal_turns: HashSet::new(),
                        pending_permission: None,
                        pending_question: None,
                    },
                );
                vec![
                    AppEvent::SessionCreated {
                        cwd,
                        origin,
                        profile,
                    },
                    AppEvent::SecurityStatusChanged {
                        state: SecurityStatus::Unverified,
                    },
                ]
            }
            AppCommand::ResumeSession {
                session_id,
                cwd,
                origin,
                profile,
            } => {
                if self.sessions.contains_key(&session_id) {
                    return Err(AxiomError::InvalidTransition(format!(
                        "session {session_id} is already loaded"
                    )));
                }
                self.sessions.insert(
                    session_id.clone(),
                    SessionState {
                        id: session_id,
                        cwd: cwd.clone(),
                        origin,
                        profile,
                        lifecycle: SessionLifecycle::Ready,
                        active_turn: None,
                        terminal_turns: HashSet::new(),
                        pending_permission: None,
                        pending_question: None,
                    },
                );
                vec![AppEvent::SessionResumed {
                    cwd,
                    origin,
                    profile,
                }]
            }
            AppCommand::SubmitPrompt {
                session_id,
                turn_id,
                text,
                attachments,
            } => {
                axiom_inference::validate_prompt(&text, &attachments)
                    .map_err(|error| AxiomError::InvalidTransition(error.to_string()))?;
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(format!(
                        "session is {:?}, not ready",
                        session.lifecycle
                    )));
                }
                if session.terminal_turns.contains(&turn_id) {
                    return Err(AxiomError::InvalidTransition(format!(
                        "turn {turn_id} is already terminal and cannot run again"
                    )));
                }
                session.lifecycle = SessionLifecycle::Running;
                session.active_turn = Some(turn_id.clone());
                vec![
                    AppEvent::PromptAccepted {
                        turn_id: turn_id.clone(),
                        text,
                        attachments,
                    },
                    AppEvent::TurnStarted { turn_id },
                ]
            }
            AppCommand::CancelTurn {
                session_id,
                turn_id,
            } => {
                self.finish_active(&session_id, &turn_id)?;
                vec![AppEvent::TurnCancelled { turn_id }]
            }
            AppCommand::FinishTurn {
                session_id,
                turn_id,
            } => {
                self.finish_active(&session_id, &turn_id)?;
                vec![AppEvent::TurnCompleted { turn_id }]
            }
            AppCommand::FailTurn {
                session_id,
                turn_id,
                message,
            } => {
                self.finish_active(&session_id, &turn_id)?;
                vec![AppEvent::ErrorRaised {
                    turn_id: Some(turn_id),
                    message,
                }]
            }
            AppCommand::ConfigureDesktopAgent {
                session_id,
                settings,
            } => {
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(
                        "Agent settings may change only while the thread is idle".into(),
                    ));
                }
                session.cwd = PathBuf::from(&settings.working_directory);
                session.profile = PermissionProfile::for_desktop_agent(&settings);
                vec![AppEvent::DesktopAgentConfigured { settings }]
            }
            AppCommand::ChangePermissionProfile {
                session_id,
                profile,
            } => {
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(
                        "permission profile may change only while the session is ready".into(),
                    ));
                }
                session.profile = profile;
                vec![AppEvent::PermissionProfileChanged { profile }]
            }
            AppCommand::ChangeModel { session_id, model } => {
                if model.trim().is_empty() || model.chars().any(char::is_whitespace) {
                    return Err(AxiomError::InvalidTransition(
                        "model ID must be a non-empty value without whitespace".into(),
                    ));
                }
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(
                        "model may change only while the session is ready".into(),
                    ));
                }
                vec![AppEvent::ModelChanged { model }]
            }
            AppCommand::ChangeModelSettings {
                session_id,
                model,
                thinking,
                reset_security,
            } => {
                if model.trim().is_empty() || model.chars().any(char::is_whitespace) {
                    return Err(AxiomError::InvalidTransition(
                        "model ID must be a non-empty value without whitespace".into(),
                    ));
                }
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(
                        "model settings may change only while the session is ready".into(),
                    ));
                }
                let mut events = vec![AppEvent::ThinkingLevelChanged { level: thinking }];
                if reset_security {
                    events.push(AppEvent::SecurityStatusChanged {
                        state: SecurityStatus::Unverified,
                    });
                }
                // Keep the user-visible result focused on the action they
                // took. Front ends apply this atomic batch in order, so the
                // model event deliberately follows reconciled thinking and
                // security invalidation and owns the final status message.
                events.push(AppEvent::ModelChanged { model });
                events
            }
            AppCommand::ChangeThinkingLevel { session_id, level } => {
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(
                        "thinking level may change only while the session is ready".into(),
                    ));
                }
                vec![AppEvent::ThinkingLevelChanged { level }]
            }
            AppCommand::RecordContextCompaction {
                session_id,
                summary,
                messages_before,
            } => {
                if summary.trim().is_empty() {
                    return Err(AxiomError::InvalidTransition(
                        "compaction summary cannot be empty".into(),
                    ));
                }
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(
                        "context may be compacted only while the session is ready".into(),
                    ));
                }
                vec![AppEvent::ContextCompacted {
                    summary,
                    messages_before,
                }]
            }
            AppCommand::CloseSession { session_id } => {
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(
                        "session must be ready before it can close".into(),
                    ));
                }
                session.lifecycle = SessionLifecycle::Closed;
                vec![AppEvent::SessionClosed]
            }
        };

        Ok(events
            .into_iter()
            .map(|event| self.envelope(session_id.clone(), correlation_id.clone(), origin, event))
            .collect())
    }

    #[must_use]
    pub fn session(&self, id: &SessionId) -> Option<&SessionState> {
        self.sessions.get(id)
    }

    pub fn record(
        &mut self,
        session_id: SessionId,
        correlation_id: CorrelationId,
        origin: Origin,
        event: AppEvent,
    ) -> Result<EventEnvelope> {
        let session = self.session_mut(&session_id)?;
        match &event {
            AppEvent::SteeringApplied { turn_id, .. } => {
                if session.active_turn.as_ref() != Some(turn_id)
                    || session.lifecycle != SessionLifecycle::Running
                {
                    return Err(AxiomError::InvalidTransition(
                        "steering requires the active running turn".into(),
                    ));
                }
            }
            AppEvent::PermissionRequired { request_id, .. } => {
                if session.lifecycle != SessionLifecycle::Running {
                    return Err(AxiomError::InvalidTransition(
                        "permission can be requested only during a running turn".into(),
                    ));
                }
                session.lifecycle = SessionLifecycle::WaitingForApproval;
                session.pending_permission = Some(request_id.clone());
            }
            AppEvent::PermissionResolved { request_id, .. } => {
                if session.lifecycle != SessionLifecycle::WaitingForApproval
                    || session.pending_permission.as_deref() != Some(request_id)
                {
                    return Err(AxiomError::InvalidTransition(format!(
                        "permission {request_id} is not pending"
                    )));
                }
                session.lifecycle = SessionLifecycle::Running;
                session.pending_permission = None;
            }
            AppEvent::QuestionAsked { question_id, .. } => {
                if session.lifecycle != SessionLifecycle::Running {
                    return Err(AxiomError::InvalidTransition(
                        "question can be asked only during a running turn".into(),
                    ));
                }
                session.lifecycle = SessionLifecycle::WaitingForAnswer;
                session.pending_question = Some(question_id.clone());
            }
            AppEvent::QuestionsAsked { request } => {
                if session.lifecycle != SessionLifecycle::Running || request.questions.is_empty() {
                    return Err(AxiomError::InvalidTransition(
                        "structured questions require a running turn and a non-empty request"
                            .into(),
                    ));
                }
                session.lifecycle = SessionLifecycle::WaitingForAnswer;
                session.pending_question = Some(request.request_id.clone());
            }
            AppEvent::QuestionAnswered {
                question_id,
                answers,
            } => {
                if answers.is_empty()
                    || session.lifecycle != SessionLifecycle::WaitingForAnswer
                    || session.pending_question.as_deref() != Some(question_id)
                {
                    return Err(AxiomError::InvalidTransition(format!(
                        "question {question_id} is not pending or has no answer"
                    )));
                }
                session.lifecycle = SessionLifecycle::Running;
                session.pending_question = None;
            }
            AppEvent::QuestionsAnswered {
                request_id,
                answers: _,
            }
            | AppEvent::QuestionsFailed { request_id, .. } => {
                if session.lifecycle != SessionLifecycle::WaitingForAnswer
                    || session.pending_question.as_deref() != Some(request_id)
                {
                    return Err(AxiomError::InvalidTransition(format!(
                        "question request {request_id} is not pending"
                    )));
                }
                session.lifecycle = SessionLifecycle::Running;
                session.pending_question = None;
            }
            _ => {}
        }
        Ok(self.envelope(session_id, correlation_id, origin, event))
    }

    pub fn restore_from_events(&mut self, events: &[EventEnvelope]) -> Result<SessionState> {
        let mut projection = RestoreProjection::new(events)?;
        for envelope in events {
            projection.replay(envelope)?;
        }
        Ok(self.install_restore(projection.finish()?))
    }

    fn install_restore(&mut self, restored: CompletedRestore) -> SessionState {
        self.next_sequence = self.next_sequence.max(restored.max_sequence);
        self.sessions.insert(restored.id, restored.state.clone());
        restored.state
    }

    fn session_mut(&mut self, id: &SessionId) -> Result<&mut SessionState> {
        self.sessions
            .get_mut(id)
            .ok_or_else(|| AxiomError::SessionNotFound(id.to_string()))
    }

    fn finish_active(&mut self, session_id: &SessionId, turn_id: &TurnId) -> Result<()> {
        let session = self.session_mut(session_id)?;
        if session.active_turn.as_ref() != Some(turn_id)
            || !matches!(
                session.lifecycle,
                SessionLifecycle::Running
                    | SessionLifecycle::WaitingForApproval
                    | SessionLifecycle::WaitingForAnswer
            )
        {
            return Err(AxiomError::InvalidTransition(format!(
                "turn {turn_id} is not active"
            )));
        }
        session.lifecycle = SessionLifecycle::Ready;
        session.active_turn = None;
        session.terminal_turns.insert(turn_id.clone());
        session.pending_permission = None;
        session.pending_question = None;
        Ok(())
    }

    fn envelope(
        &mut self,
        session_id: SessionId,
        correlation_id: CorrelationId,
        origin: Origin,
        event: AppEvent,
    ) -> EventEnvelope {
        self.next_sequence += 1;
        EventEnvelope {
            schema_version: 1,
            sequence: self.next_sequence,
            occurred_at: self.services.now(),
            correlation_id,
            origin,
            session_id,
            event,
        }
    }
}

fn replay_event(state: &mut Option<SessionState>, id: &SessionId, event: &AppEvent) -> Result<()> {
    match event {
        AppEvent::SessionCreated {
            cwd,
            origin,
            profile,
        }
        | AppEvent::SessionResumed {
            cwd,
            origin,
            profile,
        } => {
            if state.is_none() {
                *state = Some(SessionState {
                    id: id.clone(),
                    cwd: cwd.clone(),
                    origin: *origin,
                    profile: *profile,
                    lifecycle: SessionLifecycle::Ready,
                    active_turn: None,
                    terminal_turns: HashSet::new(),
                    pending_permission: None,
                    pending_question: None,
                });
            }
        }
        AppEvent::TurnStarted { turn_id } => {
            let session = state.as_mut().ok_or_else(|| {
                AxiomError::InvalidTransition("turn precedes session creation".into())
            })?;
            session.lifecycle = SessionLifecycle::Running;
            session.active_turn = Some(turn_id.clone());
        }
        AppEvent::PermissionRequired { request_id, .. } => {
            let session = state.as_mut().ok_or_else(|| {
                AxiomError::InvalidTransition("permission precedes session creation".into())
            })?;
            session.lifecycle = SessionLifecycle::WaitingForApproval;
            session.pending_permission = Some(request_id.clone());
        }
        AppEvent::PermissionResolved { .. } => {
            if let Some(session) = state.as_mut() {
                session.lifecycle = SessionLifecycle::Running;
                session.pending_permission = None;
            }
        }
        AppEvent::QuestionAsked { question_id, .. } => {
            let session = state.as_mut().ok_or_else(|| {
                AxiomError::InvalidTransition("question precedes session creation".into())
            })?;
            session.lifecycle = SessionLifecycle::WaitingForAnswer;
            session.pending_question = Some(question_id.clone());
        }
        AppEvent::QuestionsAsked { request } => {
            let session = state.as_mut().ok_or_else(|| {
                AxiomError::InvalidTransition("questions precede session creation".into())
            })?;
            session.lifecycle = SessionLifecycle::WaitingForAnswer;
            session.pending_question = Some(request.request_id.clone());
        }
        AppEvent::QuestionAnswered { .. }
        | AppEvent::QuestionsAnswered { .. }
        | AppEvent::QuestionsFailed { .. } => {
            if let Some(session) = state.as_mut() {
                session.lifecycle = SessionLifecycle::Running;
                session.pending_question = None;
            }
        }
        AppEvent::TurnCompleted { .. }
        | AppEvent::TurnCancelled { .. }
        | AppEvent::ErrorRaised {
            turn_id: Some(_), ..
        } => {
            if let Some(session) = state.as_mut() {
                let terminal_turn = match event {
                    AppEvent::TurnCompleted { turn_id }
                    | AppEvent::TurnCancelled { turn_id }
                    | AppEvent::ErrorRaised {
                        turn_id: Some(turn_id),
                        ..
                    } => Some(turn_id.clone()),
                    _ => None,
                };
                session.lifecycle = SessionLifecycle::Ready;
                session.active_turn = None;
                if let Some(turn_id) = terminal_turn {
                    session.terminal_turns.insert(turn_id);
                }
                session.pending_permission = None;
                session.pending_question = None;
            }
        }
        AppEvent::DesktopAgentConfigured { settings } => {
            if let Some(session) = state.as_mut() {
                session.cwd = PathBuf::from(&settings.working_directory);
                session.profile = PermissionProfile::for_desktop_agent(settings);
            }
        }
        AppEvent::PermissionProfileChanged { profile } => {
            if let Some(session) = state.as_mut() {
                session.profile = *profile;
            }
        }
        AppEvent::SessionClosed => {
            if let Some(session) = state.as_mut() {
                session.lifecycle = SessionLifecycle::Closed;
            }
        }
        _ => {}
    }
    Ok(())
}

struct RestoreProjection {
    id: SessionId,
    state: Option<SessionState>,
    max_sequence: u64,
}

struct CompletedRestore {
    id: SessionId,
    state: SessionState,
    max_sequence: u64,
}

impl RestoreProjection {
    fn new(events: &[EventEnvelope]) -> Result<Self> {
        let first = events.first().ok_or_else(|| {
            AxiomError::InvalidTransition("cannot restore an empty event stream".into())
        })?;
        Ok(Self {
            id: first.session_id.clone(),
            state: None,
            max_sequence: 0,
        })
    }

    fn replay(&mut self, envelope: &EventEnvelope) -> Result<()> {
        if envelope.session_id != self.id {
            return Err(AxiomError::InvalidTransition(
                "restore stream contains multiple sessions".into(),
            ));
        }
        self.max_sequence = self.max_sequence.max(envelope.sequence);
        replay_event(&mut self.state, &self.id, &envelope.event)
    }

    fn finish(self) -> Result<CompletedRestore> {
        let mut state = self.state.ok_or_else(|| {
            AxiomError::InvalidTransition("restore stream has no session creation".into())
        })?;
        // A close event records that the previous front end shut down; it does
        // not tombstone a durable session. Unknown in-flight work is never
        // replayed. Every successfully restored session resumes ready and the
        // durable turn records retain interrupted evidence for recovery.
        state.lifecycle = SessionLifecycle::Ready;
        state.active_turn = None;
        state.pending_permission = None;
        state.pending_question = None;
        Ok(CompletedRestore {
            id: self.id,
            state,
            max_sequence: self.max_sequence,
        })
    }
}

#[derive(Clone)]
pub struct Runtime {
    kernel: Arc<Mutex<Kernel>>,
    events: broadcast::Sender<EventEnvelope>,
    services: Arc<dyn RuntimeServices>,
}

impl Runtime {
    #[must_use]
    pub fn new(event_capacity: usize) -> Self {
        Self::with_services(event_capacity, Arc::new(SystemServices))
    }

    #[must_use]
    pub fn with_services(event_capacity: usize, services: Arc<dyn RuntimeServices>) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        Self {
            kernel: Arc::new(Mutex::new(Kernel::with_services(services.clone()))),
            events,
            services,
        }
    }

    #[must_use]
    pub fn session_id(&self) -> SessionId {
        SessionId::from_uuid(self.services.next_uuid())
    }

    #[must_use]
    pub fn turn_id(&self) -> TurnId {
        TurnId::from_uuid(self.services.next_uuid())
    }

    pub async fn dispatch(&self, command: AppCommand) -> Result<Vec<EventEnvelope>> {
        let origin = match &command {
            AppCommand::CreateSession { origin, .. } | AppCommand::ResumeSession { origin, .. } => {
                *origin
            }
            _ => self
                .kernel
                .lock()
                .await
                .session(command.session_id())
                .map_or(Origin::Test, |session| session.origin),
        };
        self.dispatch_request(CommandEnvelope {
            correlation_id: CorrelationId::from_uuid(self.services.next_uuid()),
            origin,
            cancellation: CancellationToken::new(),
            command,
        })
        .await
    }

    pub async fn dispatch_request(&self, request: CommandEnvelope) -> Result<Vec<EventEnvelope>> {
        let events = self.kernel.lock().await.apply(request)?;
        for event in &events {
            let _ = self.events.send(event.clone());
        }
        Ok(events)
    }

    pub async fn emit(&self, session_id: SessionId, event: AppEvent) -> Result<EventEnvelope> {
        let (origin, correlation_id) = {
            let kernel = self.kernel.lock().await;
            let origin = kernel
                .session(&session_id)
                .ok_or_else(|| AxiomError::SessionNotFound(session_id.to_string()))?
                .origin;
            (origin, CorrelationId::from_uuid(self.services.next_uuid()))
        };
        let event = self
            .kernel
            .lock()
            .await
            .record(session_id, correlation_id, origin, event)?;
        let _ = self.events.send(event.clone());
        Ok(event)
    }

    pub async fn restore(&self, events: &[EventEnvelope]) -> Result<SessionState> {
        self.restore_with_cooperation(events, tokio::task::yield_now)
            .await
    }

    async fn restore_with_cooperation<Cooperate, Cooperation>(
        &self,
        events: &[EventEnvelope],
        mut cooperate: Cooperate,
    ) -> Result<SessionState>
    where
        Cooperate: FnMut() -> Cooperation,
        Cooperation: Future<Output = ()>,
    {
        // Projection is deliberately independent of the shared kernel. Other
        // sessions remain dispatchable while a large journal is validated and
        // replayed; only the final state/sequence installation takes the lock.
        let mut projection = RestoreProjection::new(events)?;
        for (index, envelope) in events.iter().enumerate() {
            projection.replay(envelope)?;
            if (index + 1).is_multiple_of(RESTORE_COOPERATION_INTERVAL) {
                cooperate().await;
            }
        }
        let restored = projection.finish()?;
        let mut kernel = self.kernel.lock().await;
        Ok(kernel.install_restore(restored))
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use chrono::TimeZone as _;
    use tokio::sync::Notify;

    use super::*;

    #[test]
    fn thinking_levels_are_explicit() {
        assert_eq!(
            serde_json::from_str::<ThinkingLevel>("\"medium\"").expect("thinking"),
            ThinkingLevel::Medium
        );
        assert_eq!(
            serde_json::to_string(&ThinkingLevel::Medium).expect("explicit thinking"),
            "\"medium\""
        );
        assert!(serde_json::from_str::<ThinkingLevel>("\"auto\"").is_err());
        assert!(ThinkingLevel::from_str("auto").is_err());
        assert!(ThinkingLevel::from_str("extra-high").is_err());
        assert!(ThinkingLevel::from_str("extra_high").is_err());
    }

    fn services() -> Arc<DeterministicServices> {
        Arc::new(DeterministicServices::new(
            Utc.with_ymd_and_hms(2026, 8, 22, 0, 0, 0)
                .single()
                .expect("epoch"),
        ))
    }

    fn envelope(command: AppCommand, number: u128) -> CommandEnvelope {
        CommandEnvelope {
            correlation_id: CorrelationId::from_uuid(Uuid::from_u128(number)),
            origin: Origin::Test,
            cancellation: CancellationToken::new(),
            command,
        }
    }

    fn journal_event(session_id: &SessionId, sequence: u64, event: AppEvent) -> EventEnvelope {
        EventEnvelope {
            schema_version: 1,
            sequence,
            occurred_at: Utc
                .with_ymd_and_hms(2026, 8, 22, 0, 0, 0)
                .single()
                .expect("event time"),
            correlation_id: CorrelationId::from_uuid(Uuid::from_u128(u128::from(sequence))),
            origin: Origin::Test,
            session_id: session_id.clone(),
            event,
        }
    }

    fn create(kernel: &mut Kernel) -> SessionId {
        let session_id = SessionId::from_uuid(Uuid::from_u128(100));
        kernel
            .apply(envelope(
                AppCommand::CreateSession {
                    session_id: session_id.clone(),
                    cwd: PathBuf::from("/tmp/example"),
                    origin: Origin::Test,
                    profile: PermissionProfile::Confirm,
                },
                1,
            ))
            .expect("create session");
        session_id
    }

    #[test]
    fn a_new_session_never_claims_an_unattested_development_transport() {
        let mut kernel = Kernel::with_services(services());
        let session_id = SessionId::from_uuid(Uuid::from_u128(101));
        let events = kernel
            .apply(envelope(
                AppCommand::CreateSession {
                    session_id,
                    cwd: PathBuf::from("/tmp/example"),
                    origin: Origin::Tui,
                    profile: PermissionProfile::Confirm,
                },
                1,
            ))
            .expect("create session");
        assert!(matches!(
            events.get(1).map(|event| &event.event),
            Some(AppEvent::SecurityStatusChanged {
                state: SecurityStatus::Unverified
            })
        ));
    }

    #[test]
    fn session_setting_and_compaction_commands_are_validated_and_emitted() {
        let mut kernel = Kernel::with_services(services());
        let session_id = create(&mut kernel);
        assert!(
            kernel
                .apply(envelope(
                    AppCommand::ChangeModel {
                        session_id: session_id.clone(),
                        model: "bad model".into(),
                    },
                    2,
                ))
                .is_err()
        );
        let model = kernel
            .apply(envelope(
                AppCommand::ChangeModel {
                    session_id: session_id.clone(),
                    model: "next-model".into(),
                },
                3,
            ))
            .expect("model");
        assert!(matches!(
            model[0].event,
            AppEvent::ModelChanged { ref model } if model == "next-model"
        ));
        let thinking = kernel
            .apply(envelope(
                AppCommand::ChangeThinkingLevel {
                    session_id: session_id.clone(),
                    level: ThinkingLevel::High,
                },
                4,
            ))
            .expect("thinking");
        assert!(matches!(
            thinking[0].event,
            AppEvent::ThinkingLevelChanged {
                level: ThinkingLevel::High
            }
        ));
        let compacted = kernel
            .apply(envelope(
                AppCommand::RecordContextCompaction {
                    session_id,
                    summary: "successor".into(),
                    messages_before: 12,
                },
                5,
            ))
            .expect("compaction");
        assert!(matches!(
            compacted[0].event,
            AppEvent::ContextCompacted {
                messages_before: 12,
                ..
            }
        ));
    }

    #[test]
    fn transition_table_rejects_invalid_ready_running_waiting_and_closed_paths() {
        let mut kernel = Kernel::with_services(services());
        let session_id = create(&mut kernel);
        let turn_id = TurnId::from_uuid(Uuid::from_u128(200));

        assert!(
            kernel
                .apply(envelope(
                    AppCommand::FinishTurn {
                        session_id: session_id.clone(),
                        turn_id: turn_id.clone()
                    },
                    2
                ))
                .is_err()
        );
        kernel
            .apply(envelope(
                AppCommand::SubmitPrompt {
                    attachments: Vec::new(),
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                    text: "hello".into(),
                },
                3,
            ))
            .expect("submit");
        assert!(
            kernel
                .apply(envelope(
                    AppCommand::SubmitPrompt {
                        attachments: Vec::new(),
                        session_id: session_id.clone(),
                        turn_id: TurnId::new(),
                        text: "overlap".into()
                    },
                    4
                ))
                .is_err()
        );
        assert!(
            kernel
                .apply(envelope(
                    AppCommand::ChangePermissionProfile {
                        session_id: session_id.clone(),
                        profile: PermissionProfile::Observe
                    },
                    5
                ))
                .is_err()
        );

        kernel
            .record(
                session_id.clone(),
                CorrelationId::new(),
                Origin::Test,
                AppEvent::PermissionRequired {
                    request_id: "approval".into(),
                    explanation: "write x".into(),
                    effect: serde_json::json!({"kind":"file_write","path":"x"}),
                    choices: vec!["allow_once".into(), "deny".into()],
                },
            )
            .expect("pending");
        assert_eq!(
            kernel.session(&session_id).expect("state").lifecycle,
            SessionLifecycle::WaitingForApproval
        );
        kernel
            .record(
                session_id.clone(),
                CorrelationId::new(),
                Origin::Test,
                AppEvent::PermissionResolved {
                    request_id: "approval".into(),
                    allowed: false,
                    choice: Some("deny".into()),
                },
            )
            .expect("resolve");
        kernel
            .apply(envelope(
                AppCommand::FinishTurn {
                    session_id: session_id.clone(),
                    turn_id: turn_id.clone(),
                },
                8,
            ))
            .expect("finish");
        assert!(
            kernel
                .apply(envelope(
                    AppCommand::FinishTurn {
                        session_id: session_id.clone(),
                        turn_id
                    },
                    9
                ))
                .is_err()
        );
        kernel
            .apply(envelope(
                AppCommand::CloseSession {
                    session_id: session_id.clone(),
                },
                10,
            ))
            .expect("close");
        assert!(
            kernel
                .apply(envelope(
                    AppCommand::SubmitPrompt {
                        attachments: Vec::new(),
                        session_id,
                        turn_id: TurnId::new(),
                        text: "after close".into()
                    },
                    11
                ))
                .is_err()
        );
    }

    #[test]
    fn command_state_matrix_is_fail_closed() {
        #[derive(Clone, Copy)]
        enum Candidate {
            Submit,
            Finish,
            Fail,
            Cancel,
            Profile,
            Close,
        }
        let states = [
            SessionLifecycle::Ready,
            SessionLifecycle::Running,
            SessionLifecycle::WaitingForApproval,
            SessionLifecycle::WaitingForAnswer,
            SessionLifecycle::Closed,
        ];
        let candidates = [
            Candidate::Submit,
            Candidate::Finish,
            Candidate::Fail,
            Candidate::Cancel,
            Candidate::Profile,
            Candidate::Close,
        ];
        for state in states {
            for candidate in candidates {
                let mut kernel = Kernel::with_services(services());
                let session = create(&mut kernel);
                let turn = TurnId::from_uuid(Uuid::from_u128(400));
                {
                    let current = kernel.sessions.get_mut(&session).expect("session");
                    current.lifecycle = state;
                    current.active_turn =
                        (!matches!(state, SessionLifecycle::Ready | SessionLifecycle::Closed))
                            .then(|| turn.clone());
                    current.pending_permission =
                        (state == SessionLifecycle::WaitingForApproval).then(|| "approval".into());
                    current.pending_question =
                        (state == SessionLifecycle::WaitingForAnswer).then(|| "question".into());
                }
                let command = match candidate {
                    Candidate::Submit => AppCommand::SubmitPrompt {
                        attachments: Vec::new(),
                        session_id: session.clone(),
                        turn_id: TurnId::new(),
                        text: "next".into(),
                    },
                    Candidate::Finish => AppCommand::FinishTurn {
                        session_id: session.clone(),
                        turn_id: turn.clone(),
                    },
                    Candidate::Fail => AppCommand::FailTurn {
                        session_id: session.clone(),
                        turn_id: turn.clone(),
                        message: "fixture failure".into(),
                    },
                    Candidate::Cancel => AppCommand::CancelTurn {
                        session_id: session.clone(),
                        turn_id: turn.clone(),
                    },
                    Candidate::Profile => AppCommand::ChangePermissionProfile {
                        session_id: session.clone(),
                        profile: PermissionProfile::Observe,
                    },
                    Candidate::Close => AppCommand::CloseSession {
                        session_id: session.clone(),
                    },
                };
                let expected = matches!(
                    (state, candidate),
                    (
                        SessionLifecycle::Ready,
                        Candidate::Submit | Candidate::Profile | Candidate::Close,
                    ) | (
                        SessionLifecycle::Running
                            | SessionLifecycle::WaitingForApproval
                            | SessionLifecycle::WaitingForAnswer,
                        Candidate::Finish | Candidate::Fail | Candidate::Cancel,
                    )
                );
                assert_eq!(
                    kernel.apply(envelope(command, 500)).is_ok(),
                    expected,
                    "unexpected result for {state:?}"
                );
            }
        }
    }

    #[test]
    fn failed_question_is_terminal_and_later_interactions_can_continue() {
        let mut kernel = Kernel::with_services(services());
        let session = create(&mut kernel);
        let turn = TurnId::new();
        kernel
            .apply(envelope(
                AppCommand::SubmitPrompt {
                    attachments: Vec::new(),
                    session_id: session.clone(),
                    turn_id: turn,
                    text: "work".into(),
                },
                1,
            ))
            .expect("submit");
        kernel
            .record(
                session.clone(),
                CorrelationId::new(),
                Origin::Test,
                AppEvent::QuestionsAsked {
                    request: QuestionRequest {
                        request_id: "question".into(),
                        questions: vec![QuestionSpec {
                            id: "choice".into(),
                            prompt: "Choose".into(),
                            options: vec!["one".into()],
                            multiple: false,
                            required: true,
                        }],
                    },
                },
            )
            .expect("question");
        kernel
            .record(
                session.clone(),
                CorrelationId::new(),
                Origin::Test,
                AppEvent::QuestionsFailed {
                    request_id: "question".into(),
                    reason: "client unsupported".into(),
                },
            )
            .expect("terminal question failure");
        kernel
            .record(
                session,
                CorrelationId::new(),
                Origin::Test,
                AppEvent::PermissionRequired {
                    request_id: "approval".into(),
                    explanation: "continue".into(),
                    effect: serde_json::json!({"kind":"network"}),
                    choices: vec!["allow_once".into(), "deny".into()],
                },
            )
            .expect("later interaction");
    }

    proptest::proptest! {
        #[test]
        fn terminal_turn_id_never_becomes_active_again(cancel in proptest::bool::ANY) {
            let mut kernel = Kernel::with_services(services());
            let session = create(&mut kernel);
            let terminal_turn = TurnId::from_uuid(Uuid::from_u128(600));
            kernel.apply(envelope(AppCommand::SubmitPrompt {attachments: Vec::new(),  session_id: session.clone(), turn_id: terminal_turn.clone(), text: "work".into() }, 601)).expect("submit");
            let terminal = if cancel {
                AppCommand::CancelTurn { session_id: session.clone(), turn_id: terminal_turn.clone() }
            } else {
                AppCommand::FinishTurn { session_id: session.clone(), turn_id: terminal_turn.clone() }
            };
            kernel.apply(envelope(terminal, 602)).expect("terminal");
            let replay = AppCommand::FinishTurn {
                session_id: session.clone(),
                turn_id: terminal_turn.clone(),
            };
            proptest::prop_assert!(kernel.apply(envelope(replay, 603)).is_err());

            let rerun = AppCommand::SubmitPrompt {attachments: Vec::new(),
                session_id: session,
                turn_id: terminal_turn,
                text: "attempt to revive a terminal turn".into(),
            };
            proptest::prop_assert!(kernel.apply(envelope(rerun, 604)).is_err());
        }
    }

    #[test]
    fn cancelled_command_has_no_state_or_event_effect() {
        let mut kernel = Kernel::with_services(services());
        let token = CancellationToken::new();
        token.cancel();
        let id = SessionId::new();
        let result = kernel.apply(CommandEnvelope {
            correlation_id: CorrelationId::new(),
            origin: Origin::Test,
            cancellation: token,
            command: AppCommand::CreateSession {
                session_id: id.clone(),
                cwd: PathBuf::from("."),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
            },
        });
        assert!(matches!(result, Err(AxiomError::Cancelled)));
        assert!(kernel.session(&id).is_none());
    }

    #[test]
    fn golden_command_stream_is_reproducible() {
        fn run() -> Vec<EventEnvelope> {
            let mut kernel = Kernel::with_services(services());
            let session = SessionId::from_uuid(Uuid::from_u128(20));
            let turn = TurnId::from_uuid(Uuid::from_u128(21));
            let mut events = kernel
                .apply(envelope(
                    AppCommand::CreateSession {
                        session_id: session.clone(),
                        cwd: PathBuf::from("/work"),
                        origin: Origin::Test,
                        profile: PermissionProfile::Observe,
                    },
                    30,
                ))
                .expect("create");
            events.extend(
                kernel
                    .apply(envelope(
                        AppCommand::SubmitPrompt {
                            attachments: Vec::new(),
                            session_id: session.clone(),
                            turn_id: turn.clone(),
                            text: "inspect".into(),
                        },
                        31,
                    ))
                    .expect("prompt"),
            );
            events.extend(
                kernel
                    .apply(envelope(
                        AppCommand::FinishTurn {
                            session_id: session,
                            turn_id: turn,
                        },
                        32,
                    ))
                    .expect("finish"),
            );
            events
        }
        assert_eq!(run(), run());
    }

    #[test]
    fn replay_marks_inflight_work_ready_without_replaying_it() {
        let mut source = Kernel::with_services(services());
        let session = create(&mut source);
        let turn = TurnId::new();
        let events = source
            .apply(envelope(
                AppCommand::SubmitPrompt {
                    attachments: Vec::new(),
                    session_id: session.clone(),
                    turn_id: turn,
                    text: "work".into(),
                },
                20,
            ))
            .expect("submit");
        let mut all = Vec::new();
        let mut initial = Kernel::with_services(services())
            .apply(envelope(
                AppCommand::CreateSession {
                    session_id: session.clone(),
                    cwd: PathBuf::from("/tmp/example"),
                    origin: Origin::Test,
                    profile: PermissionProfile::Confirm,
                },
                1,
            ))
            .expect("initial");
        all.append(&mut initial);
        all.extend(events);
        let mut restored = Kernel::with_services(services());
        let state = restored.restore_from_events(&all).expect("restore");
        assert_eq!(state.lifecycle, SessionLifecycle::Ready);
        assert!(state.active_turn.is_none());
    }

    #[tokio::test]
    async fn slow_or_dropped_subscribers_do_not_block_dispatch() {
        let runtime = Runtime::with_services(1, services());
        let _slow = runtime.subscribe();
        let id = runtime.session_id();
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            runtime.dispatch(AppCommand::CreateSession {
                session_id: id,
                cwd: PathBuf::from("."),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
            }),
        )
        .await
        .expect("not blocked")
        .expect("dispatch");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn runtime_restore_is_cooperative_unlocked_and_equivalent_to_kernel_replay() {
        let restored_id = SessionId::from_uuid(Uuid::from_u128(900));
        let mut events = vec![journal_event(
            &restored_id,
            1,
            AppEvent::SessionCreated {
                cwd: PathBuf::from("/tmp/large-restore"),
                origin: Origin::Test,
                profile: PermissionProfile::Observe,
            },
        )];
        for sequence in 2..=RESTORE_COOPERATION_INTERVAL as u64 * 4 + 1 {
            events.push(journal_event(
                &restored_id,
                sequence,
                AppEvent::PermissionProfileChanged {
                    profile: if sequence.is_multiple_of(2) {
                        PermissionProfile::Confirm
                    } else {
                        PermissionProfile::Observe
                    },
                },
            ));
        }
        let max_sequence = events.last().expect("last event").sequence;
        let mut expected_kernel = Kernel::with_services(services());
        let expected = expected_kernel
            .restore_from_events(&events)
            .expect("reference restore");

        let runtime = Runtime::with_services(8, services());
        let other_id = runtime.session_id();
        runtime
            .dispatch(AppCommand::CreateSession {
                session_id: other_id.clone(),
                cwd: PathBuf::from("/tmp/other-session"),
                origin: Origin::Test,
                profile: PermissionProfile::Observe,
            })
            .await
            .expect("create other session");

        let reached_checkpoint = Arc::new(Notify::new());
        let release_checkpoint = Arc::new(Notify::new());
        let first_checkpoint = Arc::new(AtomicBool::new(true));
        let restore_runtime = runtime.clone();
        let restore_events = Arc::new(events);
        let restore_task = {
            let reached_checkpoint = reached_checkpoint.clone();
            let release_checkpoint = release_checkpoint.clone();
            let first_checkpoint = first_checkpoint.clone();
            tokio::spawn(async move {
                restore_runtime
                    .restore_with_cooperation(restore_events.as_slice(), move || {
                        let is_first = first_checkpoint.swap(false, Ordering::SeqCst);
                        let reached_checkpoint = reached_checkpoint.clone();
                        let release_checkpoint = release_checkpoint.clone();
                        async move {
                            if is_first {
                                reached_checkpoint.notify_one();
                                release_checkpoint.notified().await;
                            } else {
                                tokio::task::yield_now().await;
                            }
                        }
                    })
                    .await
            })
        };

        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reached_checkpoint.notified(),
        )
        .await
        .expect("restore reached cooperative checkpoint");
        assert!(!restore_task.is_finished());
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            runtime.dispatch(AppCommand::ChangePermissionProfile {
                session_id: other_id,
                profile: PermissionProfile::Confirm,
            }),
        )
        .await
        .expect("kernel remains responsive during projection")
        .expect("other session dispatch");

        release_checkpoint.notify_one();
        let restored = restore_task
            .await
            .expect("restore task join")
            .expect("runtime restore");
        assert_eq!(restored, expected);
        let next = runtime
            .dispatch(AppCommand::ChangePermissionProfile {
                session_id: restored_id,
                profile: PermissionProfile::FullAccess,
            })
            .await
            .expect("dispatch after restore");
        assert_eq!(next[0].sequence, max_sequence + 1);
    }
}
