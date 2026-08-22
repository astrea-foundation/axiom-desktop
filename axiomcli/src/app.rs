use std::{collections::HashMap, fmt, path::PathBuf, str::FromStr, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast};
use uuid::Uuid;

use crate::{AxiomError, Result};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(input: &str) -> std::result::Result<Self, Self::Err> {
        Uuid::parse_str(input).map(Self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(Uuid);

impl TurnId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TurnId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
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
    Observe,
    #[default]
    Confirm,
    Workspace,
    Autonomous,
}

impl fmt::Display for PermissionProfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Observe => "observe",
            Self::Confirm => "confirm",
            Self::Workspace => "workspace",
            Self::Autonomous => "autonomous",
        };
        formatter.write_str(value)
    }
}

impl FromStr for PermissionProfile {
    type Err = AxiomError;

    fn from_str(input: &str) -> Result<Self> {
        match input {
            "observe" => Ok(Self::Observe),
            "confirm" => Ok(Self::Confirm),
            "workspace" => Ok(Self::Workspace),
            "autonomous" => Ok(Self::Autonomous),
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
    SubmitPrompt {
        session_id: SessionId,
        turn_id: TurnId,
        text: String,
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
    CloseSession {
        session_id: SessionId,
    },
}

impl AppCommand {
    #[must_use]
    pub fn session_id(&self) -> &SessionId {
        match self {
            Self::CreateSession { session_id, .. }
            | Self::SubmitPrompt { session_id, .. }
            | Self::CancelTurn { session_id, .. }
            | Self::FinishTurn { session_id, .. }
            | Self::FailTurn { session_id, .. }
            | Self::ChangePermissionProfile { session_id, .. }
            | Self::CloseSession { session_id } => session_id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AppEvent {
    SessionCreated {
        cwd: PathBuf,
        origin: Origin,
        profile: PermissionProfile,
    },
    PromptAccepted {
        turn_id: TurnId,
        text: String,
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
    ToolProposed {
        turn_id: TurnId,
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    PermissionRequired {
        request_id: String,
        explanation: String,
    },
    PermissionResolved {
        request_id: String,
        allowed: bool,
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
    QuestionAsked {
        question_id: String,
        prompt: String,
        options: Vec<String>,
    },
    PlanProposed {
        markdown: String,
    },
    BackgroundTaskChanged {
        task_id: String,
        state: String,
    },
    PermissionProfileChanged {
        profile: PermissionProfile,
    },
    ProviderStatusChanged {
        connected: bool,
        detail: String,
    },
    SecurityStatusChanged {
        state: SecurityStatus,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityStatus {
    UnattestedDevelopment,
    Verified,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub schema_version: u32,
    pub sequence: u64,
    pub occurred_at: DateTime<Utc>,
    pub session_id: SessionId,
    pub event: AppEvent,
}

#[derive(Clone, Debug)]
pub struct SessionState {
    pub id: SessionId,
    pub cwd: PathBuf,
    pub origin: Origin,
    pub profile: PermissionProfile,
    pub lifecycle: SessionLifecycle,
    pub active_turn: Option<TurnId>,
}

#[derive(Default)]
pub struct Kernel {
    sessions: HashMap<SessionId, SessionState>,
    next_sequence: u64,
}

impl Kernel {
    pub fn apply(&mut self, command: AppCommand) -> Result<Vec<EventEnvelope>> {
        let session_id = command.session_id().clone();
        let events = match command {
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
                        id: session_id.clone(),
                        cwd: cwd.clone(),
                        origin,
                        profile,
                        lifecycle: SessionLifecycle::Ready,
                        active_turn: None,
                    },
                );
                vec![
                    AppEvent::SessionCreated {
                        cwd,
                        origin,
                        profile,
                    },
                    AppEvent::SecurityStatusChanged {
                        state: SecurityStatus::UnattestedDevelopment,
                    },
                ]
            }
            AppCommand::SubmitPrompt {
                session_id,
                turn_id,
                text,
            } => {
                if text.trim().is_empty() {
                    return Err(AxiomError::InvalidTransition(
                        "prompt cannot be empty".into(),
                    ));
                }
                let session = self.session_mut(&session_id)?;
                if session.lifecycle != SessionLifecycle::Ready {
                    return Err(AxiomError::InvalidTransition(format!(
                        "session is {:?}, not ready",
                        session.lifecycle
                    )));
                }
                session.lifecycle = SessionLifecycle::Running;
                session.active_turn = Some(turn_id.clone());
                vec![
                    AppEvent::PromptAccepted {
                        turn_id: turn_id.clone(),
                        text,
                    },
                    AppEvent::TurnStarted { turn_id },
                ]
            }
            AppCommand::CancelTurn {
                session_id,
                turn_id,
            } => {
                let session = self.require_active_turn(&session_id, &turn_id)?;
                session.lifecycle = SessionLifecycle::Ready;
                session.active_turn = None;
                vec![AppEvent::TurnCancelled { turn_id }]
            }
            AppCommand::FinishTurn {
                session_id,
                turn_id,
            } => {
                let session = self.require_active_turn(&session_id, &turn_id)?;
                session.lifecycle = SessionLifecycle::Ready;
                session.active_turn = None;
                vec![AppEvent::TurnCompleted { turn_id }]
            }
            AppCommand::FailTurn {
                session_id,
                turn_id,
                message,
            } => {
                let session = self.require_active_turn(&session_id, &turn_id)?;
                session.lifecycle = SessionLifecycle::Ready;
                session.active_turn = None;
                vec![AppEvent::ErrorRaised {
                    turn_id: Some(turn_id),
                    message,
                }]
            }
            AppCommand::ChangePermissionProfile {
                session_id,
                profile,
            } => {
                let session = self.session_mut(&session_id)?;
                if session.lifecycle == SessionLifecycle::Closed {
                    return Err(AxiomError::InvalidTransition(
                        "closed session cannot change profile".into(),
                    ));
                }
                session.profile = profile;
                vec![AppEvent::PermissionProfileChanged { profile }]
            }
            AppCommand::CloseSession { session_id } => {
                let session = self.session_mut(&session_id)?;
                if session.active_turn.is_some() {
                    return Err(AxiomError::InvalidTransition(
                        "cancel the active turn before closing the session".into(),
                    ));
                }
                session.lifecycle = SessionLifecycle::Closed;
                vec![AppEvent::SessionClosed]
            }
        };

        Ok(events
            .into_iter()
            .map(|event| self.envelope(session_id.clone(), event))
            .collect())
    }

    #[must_use]
    pub fn session(&self, id: &SessionId) -> Option<&SessionState> {
        self.sessions.get(id)
    }

    pub fn record(&mut self, session_id: SessionId, event: AppEvent) -> Result<EventEnvelope> {
        if !self.sessions.contains_key(&session_id) {
            return Err(AxiomError::SessionNotFound(session_id.to_string()));
        }
        Ok(self.envelope(session_id, event))
    }

    fn session_mut(&mut self, id: &SessionId) -> Result<&mut SessionState> {
        self.sessions
            .get_mut(id)
            .ok_or_else(|| AxiomError::SessionNotFound(id.to_string()))
    }

    fn require_active_turn(
        &mut self,
        session_id: &SessionId,
        turn_id: &TurnId,
    ) -> Result<&mut SessionState> {
        let session = self.session_mut(session_id)?;
        if session.active_turn.as_ref() != Some(turn_id) {
            return Err(AxiomError::InvalidTransition(format!(
                "turn {turn_id} is not active"
            )));
        }
        Ok(session)
    }

    fn envelope(&mut self, session_id: SessionId, event: AppEvent) -> EventEnvelope {
        self.next_sequence += 1;
        EventEnvelope {
            schema_version: 1,
            sequence: self.next_sequence,
            occurred_at: Utc::now(),
            session_id,
            event,
        }
    }
}

#[derive(Clone)]
pub struct Runtime {
    kernel: Arc<Mutex<Kernel>>,
    events: broadcast::Sender<EventEnvelope>,
}

impl Runtime {
    #[must_use]
    pub fn new(event_capacity: usize) -> Self {
        let (events, _) = broadcast::channel(event_capacity.max(1));
        Self {
            kernel: Arc::new(Mutex::new(Kernel::default())),
            events,
        }
    }

    pub async fn dispatch(&self, command: AppCommand) -> Result<Vec<EventEnvelope>> {
        let events = self.kernel.lock().await.apply(command)?;
        for event in &events {
            let _ = self.events.send(event.clone());
        }
        Ok(events)
    }

    pub async fn emit(&self, session_id: SessionId, event: AppEvent) -> Result<EventEnvelope> {
        let event = self.kernel.lock().await.record(session_id, event)?;
        let _ = self.events.send(event.clone());
        Ok(event)
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(kernel: &mut Kernel) -> SessionId {
        let session_id = SessionId::new();
        kernel
            .apply(AppCommand::CreateSession {
                session_id: session_id.clone(),
                cwd: PathBuf::from("/tmp/example"),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
            })
            .expect("create session");
        session_id
    }

    #[test]
    fn turn_lifecycle_is_monotonic() {
        let mut kernel = Kernel::default();
        let session_id = create(&mut kernel);
        let turn_id = TurnId::new();
        kernel
            .apply(AppCommand::SubmitPrompt {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
                text: "hello".into(),
            })
            .expect("submit");
        kernel
            .apply(AppCommand::FinishTurn {
                session_id: session_id.clone(),
                turn_id: turn_id.clone(),
            })
            .expect("finish");
        assert!(
            kernel
                .apply(AppCommand::FinishTurn {
                    session_id,
                    turn_id
                })
                .is_err()
        );
    }

    #[test]
    fn cannot_overlap_turns() {
        let mut kernel = Kernel::default();
        let session_id = create(&mut kernel);
        kernel
            .apply(AppCommand::SubmitPrompt {
                session_id: session_id.clone(),
                turn_id: TurnId::new(),
                text: "first".into(),
            })
            .expect("first prompt");
        assert!(
            kernel
                .apply(AppCommand::SubmitPrompt {
                    session_id,
                    turn_id: TurnId::new(),
                    text: "second".into(),
                })
                .is_err()
        );
    }

    #[test]
    fn empty_prompt_is_rejected() {
        let mut kernel = Kernel::default();
        let session_id = create(&mut kernel);
        assert!(
            kernel
                .apply(AppCommand::SubmitPrompt {
                    session_id,
                    turn_id: TurnId::new(),
                    text: "  ".into(),
                })
                .is_err()
        );
    }

    #[tokio::test]
    async fn runtime_broadcasts_ordered_events() {
        let runtime = Runtime::new(16);
        let mut receiver = runtime.subscribe();
        let session_id = SessionId::new();
        runtime
            .dispatch(AppCommand::CreateSession {
                session_id,
                cwd: PathBuf::from("."),
                origin: Origin::Test,
                profile: PermissionProfile::Confirm,
            })
            .await
            .expect("dispatch");
        let first = receiver.recv().await.expect("first event");
        let second = receiver.recv().await.expect("second event");
        assert_eq!(first.sequence + 1, second.sequence);
    }
}
