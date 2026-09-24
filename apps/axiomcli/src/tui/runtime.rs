//! Terminal lifecycle, user input, and asynchronous application work.

use std::{
    collections::BTreeMap,
    env,
    fmt::Write as _,
    io,
    path::PathBuf,
    str::FromStr as _,
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
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
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{
    AxiomError, Result,
    agent::{
        APP_EVENT_QUEUE_CAPACITY, CompactionResult, ModelSettings, QuestionHandler,
        SecurityVerification, TurnContext, TurnRunner, reconcile_model_settings,
        reconcile_new_session_settings,
    },
    app::{
        AppCommand, AppEvent, Origin, PermissionProfile, QuestionRequest, Runtime, SessionId,
        ThinkingLevel, TurnId,
    },
    auth::{AccountStatus, NativeLogin, ValidationStatus},
    policy::{ApprovalChoice, ApprovalHandler, ApprovalRequest, ApprovalResponse},
    session::{SessionStore, model_for_resume, permission_for_resume, thinking_for_resume},
    slash::{self, SlashCommand},
};

use super::{
    ANIMATION_TICK,
    billing::{BillingAction, BillingFlow, BillingUpdate, BillingView},
    input::{approval_choice, command_allowed_while_running, is_text_input},
    screens::{composer_text_width, render, scroll_transcript_down, scroll_transcript_up},
    state::{
        AuthOverlay, EntryKind, Focus, InteractionView, Overlay, SavePickerFocus, SecurityView,
        StatusTone, TuiLaunch, TuiState,
    },
    task::TaskActivity,
    text::{is_attestation_failure, sanitize_terminal_text},
};

#[derive(Clone, Copy, Debug)]
pub(super) struct SetAxiomTerminalBackground(pub(super) Option<&'static str>);

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
pub(super) struct ResetAxiomTerminalBackground(pub(super) bool);

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

pub(super) fn is_search_shortcut(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL)
}

pub(super) fn uses_deterministic_test_runner() -> bool {
    cfg!(debug_assertions)
        && env::var("AXIOMCLI_TEST_RUNNER").is_ok_and(|value| !value.trim().is_empty())
}

pub(super) fn stop_running_turn(
    state: &mut TuiState,
    cancellation: Option<&CancellationToken>,
) -> bool {
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

pub(super) fn slash_help_text() -> String {
    let mut text = String::from("SLASH COMMANDS\n");
    for spec in slash::COMMANDS {
        let _ = writeln!(text, "{}\n  {}", spec.usage, spec.description);
    }
    text.push_str("\nCommands run locally and are never sent to the model as prompts.");
    text
}

pub(super) fn show_slash_error(state: &mut TuiState, message: &str) {
    let message = sanitize_terminal_text(message);
    state.status = format!("Command error: {message}");
    state.status_tone = StatusTone::Error;
}

pub(super) fn account_status_text(account: &AccountStatus) -> String {
    match account.source {
        crate::auth::CredentialSource::Environment => {
            format!("Using AXIOM_API_KEY for account {}.", account.account.id)
        }
        crate::auth::CredentialSource::SystemKeyring => {
            format!(
                "Signed in to account {}; its refresh token is in the system credential store.",
                account.account.id
            )
        }
    }
}

pub(super) enum UiMessage {
    UpdateAvailable(String),
    TitleFinished {
        session_id: SessionId,
        account_generation: u64,
    },
    BillingFinished {
        generation: u64,
        account_generation: u64,
        result: Result<BillingUpdate>,
    },
    AccountingRecovered {
        session_id: SessionId,
        account_generation: u64,
        result: Result<Vec<axiom_inference::RequestUsage>>,
    },
    SteeringDone {
        session_id: SessionId,
        text: String,
        applied: bool,
    },
    Event(AppEvent),
    Done(TurnId, Result<()>),
    SecurityPreflightDone(u64, Result<SecurityVerification>),
    CompactionDone(Result<CompactionResult>),
    ModelsLoaded(Result<Vec<axiom_inference::ModelInfo>>),
    Approval(ApprovalRequest, oneshot::Sender<ApprovalResponse>),
    Questions(
        QuestionRequest,
        oneshot::Sender<Result<BTreeMap<String, Vec<String>>>>,
    ),
    NativeLoginStarted(Box<Result<NativeLogin>>),
    AuthCompleted(Result<AccountStatus>),
    AuthLoggedOut(Result<()>),
    AuthValidated(ValidationPurpose, ValidationStatus),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ValidationPurpose {
    Startup,
    Login,
    Account,
}

pub(super) fn start_security_preflight(
    state: &mut TuiState,
    runner: &Arc<dyn TurnRunner>,
    ui_tx: &mpsc::Sender<UiMessage>,
    cancellation: &mut Option<CancellationToken>,
    generation: &mut u64,
) {
    start_security_check(
        state,
        runner,
        ui_tx,
        cancellation,
        generation,
        SecurityCheck::Refresh,
    );
}

enum SecurityCheck {
    Refresh,
    Prewarm,
    AcceptOutdated,
}

fn start_security_check(
    state: &mut TuiState,
    runner: &Arc<dyn TurnRunner>,
    ui_tx: &mpsc::Sender<UiMessage>,
    cancellation: &mut Option<CancellationToken>,
    generation: &mut u64,
    check: SecurityCheck,
) {
    if let Some(previous) = cancellation.take() {
        previous.cancel();
    }
    let token = CancellationToken::new();
    *cancellation = Some(token.clone());
    *generation = generation.wrapping_add(1);
    let request_generation = *generation;
    state.proof_refresh.enable();
    state.apply(&AppEvent::SecurityStatusChanged {
        state: crate::app::SecurityStatus::Verifying,
    });
    state.security_evidence = None;
    let task_runner = Arc::clone(runner);
    let model = state.model.clone();
    let result_tx = ui_tx.clone();
    tokio::spawn(async move {
        let result = match check {
            SecurityCheck::AcceptOutdated => {
                task_runner.accept_outdated_tee(&model, token.clone()).await
            }
            SecurityCheck::Refresh => task_runner.verify_security(&model, token.clone()).await,
            SecurityCheck::Prewarm => task_runner.prewarm_security(&model, token.clone()).await,
        };
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
pub(super) struct TuiApproval {
    pub(super) tx: mpsc::Sender<UiMessage>,
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
pub(super) struct TuiQuestions {
    pub(super) tx: mpsc::Sender<UiMessage>,
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

pub(super) struct PendingQuestions {
    pub(super) request: QuestionRequest,
    pub(super) index: usize,
    pub(super) answers: BTreeMap<String, Vec<String>>,
    pub(super) drafts: BTreeMap<String, String>,
    pub(super) response: oneshot::Sender<Result<BTreeMap<String, Vec<String>>>>,
}

pub(super) fn question_status(pending: &PendingQuestions) -> String {
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

pub(super) fn parse_tui_answer(
    input: &str,
    question: &crate::app::QuestionSpec,
) -> Result<Vec<String>> {
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

pub(super) fn restore_question_draft(pending: &PendingQuestions) -> String {
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

pub(super) fn approval_status(request: &ApprovalRequest) -> String {
    let mut choices = String::from("[Ctrl+Y] allow once");
    if request.allow_session_grants {
        choices.push_str("  [Ctrl+G] exact for session");
    }
    if let Some(scope) = request
        .suggested_prefix_scope
        .as_ref()
        .filter(|_| request.allow_session_grants)
    {
        let _ = write!(choices, "  [Ctrl+P] {scope} for session");
    }
    choices.push_str("  [Ctrl+N] deny");
    format!("{}  {choices}", request.explanation)
}

pub(super) struct TerminalGuard {
    pub(super) reset_background: bool,
}

impl TerminalGuard {
    pub(super) fn enter(background: Option<&'static str>) -> io::Result<Self> {
        enable_raw_mode()?;
        if let Err(error) = execute!(
            io::stdout(),
            SetAxiomTerminalBackground(background),
            EnterAlternateScreen,
            EnableBracketedPaste
        ) {
            let _ = execute!(
                io::stdout(),
                DisableBracketedPaste,
                LeaveAlternateScreen,
                ResetAxiomTerminalBackground(background.is_some())
            );
            let _ = disable_raw_mode();
            return Err(error);
        }
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

pub(super) async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use std::future::pending;
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = signal(SignalKind::terminate()).ok();
        let mut hangup = signal(SignalKind::hangup()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = async {
                if let Some(signal) = &mut terminate {
                    let _ = signal.recv().await;
                } else {
                    pending::<()>().await;
                }
            } => {}
            () = async {
                if let Some(signal) = &mut hangup {
                    let _ = signal.recv().await;
                } else {
                    pending::<()>().await;
                }
            } => {}
        }
    }

    #[cfg(windows)]
    {
        use std::future::pending;
        use tokio::signal::windows::{ctrl_break, ctrl_close, ctrl_shutdown};

        let mut close = ctrl_close().ok();
        let mut shutdown = ctrl_shutdown().ok();
        let mut break_signal = ctrl_break().ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = async {
                if let Some(signal) = &mut close {
                    let _ = signal.recv().await;
                } else {
                    pending::<()>().await;
                }
            } => {}
            () = async {
                if let Some(signal) = &mut shutdown {
                    let _ = signal.recv().await;
                } else {
                    pending::<()>().await;
                }
            } => {}
            () = async {
                if let Some(signal) = &mut break_signal {
                    let _ = signal.recv().await;
                } else {
                    pending::<()>().await;
                }
            } => {}
        }
    }
}

pub(super) async fn restore_tui_session(
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
    let models = runner
        .available_model_details(CancellationToken::new())
        .await?;
    let model_available = models.iter().any(|model| model.id == resumed_model);
    let reconciled_settings = if model_available {
        crate::agent::reconcile_new_session_settings(
            models.clone(),
            Some(&resumed_model),
            &state.model,
            resumed_thinking,
        )?
    } else {
        ModelSettings {
            model: resumed_model.clone(),
            thinking: resumed_thinking,
            supports_reasoning: false,
        }
    };
    state.model_details = models
        .into_iter()
        .map(|model| (model.id.clone(), model))
        .collect();
    let resumed_profile = permission_for_resume(&loaded.events, state.profile);
    runtime.restore(&loaded.events).await?;
    runner.restore_session(session_id, &loaded.events).await?;
    if model_available {
        runner
            .set_model_settings(session_id, &reconciled_settings)
            .await?;
    }
    state.profile = PermissionProfile::from_str(&summary.profile).unwrap_or(state.profile);
    for envelope in &loaded.events {
        // Older versions persisted this routine edit summary as a warning on
        // every resume. Keep the journal and WorkspaceChanged events intact,
        // but do not replay the retired synthetic notice into the transcript.
        if matches!(
            &envelope.event,
            AppEvent::WarningRaised { message }
                if message.starts_with("Workspace changes recorded in this session: ")
        ) {
            continue;
        }
        state.apply(&envelope.event);
    }
    if let Ok(snapshot) = store.thread_snapshot(session_id, None, 1) {
        let ids: Vec<_> = snapshot
            .request_usage
            .iter()
            .filter(|r| !r.settled)
            .take(100)
            .map(|r| r.request_id.clone())
            .collect();
        if !ids.is_empty()
            && let Ok(Ok(records)) = tokio::time::timeout(
                Duration::from_secs(2),
                runner.recover_accounting(&ids, CancellationToken::new()),
            )
            .await
        {
            store.reconcile_request_usage(session_id, &records)?;
        }
        let snapshot = store.thread_snapshot(session_id, None, 1)?;
        state.context_usage = snapshot.context_usage;
        for usage in snapshot.request_usage {
            state.apply(&AppEvent::RequestUsageUpdated { usage });
        }
    }
    state.model = resumed_model.clone();
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
    let model_changed = resumed_model != reconciled_settings.model;
    if model_changed || resumed_thinking != reconciled_settings.thinking {
        let corrected = runtime
            .dispatch(AppCommand::ChangeModelSettings {
                session_id: session_id.clone(),
                model: reconciled_settings.model.clone(),
                thinking: reconciled_settings.thinking,
                reset_security: model_changed,
            })
            .await?;
        store.append_all(&corrected)?;
        for event in corrected {
            state.apply(&event.event);
        }
    }
    let recovery = store.recovery_status(session_id)?;
    let mut notices = loaded.warnings;
    if recovery.interrupted_turns > 0 || !recovery.interrupted_tools.is_empty() {
        notices.push(format!(
            "Recovered {} interrupted turn(s) and {} tool/task operation(s) with unknown status; nothing was replayed.",
            recovery.interrupted_turns,
            recovery.interrupted_tools.len()
        ));
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

pub(super) fn persist_tui_thinking_change(
    store: Option<&SessionStore>,
    events: &[crate::app::EventEnvelope],
    model: &str,
    thinking: ThinkingLevel,
) -> Result<()> {
    if let Some(store) = store.filter(|store| store.is_active()) {
        store.append_all_and_set_profile_preferences(events, model, thinking)?;
    }
    Ok(())
}

pub(super) fn activate_tui_account_store(store: &SessionStore, account_id: &str) -> Result<()> {
    if store
        .active_account_id()
        .is_some_and(|active| active != account_id)
    {
        // The in-memory runtime still contains the previous account's
        // transcript. Never relabel it or let cleanup write it into the new
        // account. A fresh TUI process can safely open the newly authenticated
        // account after this one restores the terminal.
        store.deactivate_account()?;
        return Err(AxiomError::InvalidTransition(
            "the Axiom account changed in another process; restart AxiomCLI to open the new account"
                .into(),
        ));
    }
    if let Err(error) = store.activate_account(account_id) {
        let _ = store.deactivate_account();
        return Err(error);
    }
    Ok(())
}

/// Signed-out startup holds only provisional settings in memory. Discover and
/// validate the model after login, before persisting or using those settings.
pub(super) async fn finish_authenticated_bootstrap(
    runtime: &Runtime,
    runner: &Arc<dyn TurnRunner>,
    store: &SessionStore,
    session_id: &SessionId,
    state: &mut TuiState,
    pending: &mut Vec<crate::app::EventEnvelope>,
) -> Result<()> {
    let models = runner
        .available_model_details(CancellationToken::new())
        .await?;
    let preferences = store.profile_preferences()?;
    let settings = reconcile_new_session_settings(
        models.clone(),
        preferences.model.as_deref(),
        &state.model,
        preferences.thinking_level,
    )?;
    runner.set_model_settings(session_id, &settings).await?;
    let corrected = runtime
        .dispatch(AppCommand::ChangeModelSettings {
            session_id: session_id.clone(),
            model: settings.model.clone(),
            thinking: settings.thinking,
            reset_security: true,
        })
        .await?;
    pending.extend(corrected.iter().cloned());
    store.append_all_and_set_profile_preferences(pending, &settings.model, settings.thinking)?;
    pending.clear();
    state.model_details = models
        .into_iter()
        .map(|model| (model.id.clone(), model))
        .collect();
    for envelope in corrected {
        state.apply(&envelope.event);
    }
    Ok(())
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
    let mut initial_models = Vec::new();
    let (initial_settings, correct_initial_preferences) = if resume.is_none() {
        let preferences = store
            .as_ref()
            .filter(|store| store.is_active())
            .map(SessionStore::profile_preferences)
            .transpose()?;
        let settings = if store.as_ref().is_some_and(SessionStore::is_active) {
            initial_models = runner
                .available_model_details(CancellationToken::new())
                .await?;
            reconcile_new_session_settings(
                initial_models.clone(),
                preferences
                    .as_ref()
                    .and_then(|preferences| preferences.model.as_deref()),
                &default_model,
                preferences
                    .as_ref()
                    .map_or(default_thinking, |preferences| preferences.thinking_level),
            )?
        } else {
            // Signed-out startup must not perform model discovery (which would
            // require an access token) and must not open a SQLite database.
            ModelSettings {
                model: default_model.clone(),
                thinking: default_thinking,
                supports_reasoning: true,
            }
        };
        let correct_preferences = store.as_ref().is_some_and(SessionStore::is_active)
            && preferences.as_ref().is_none_or(|preferences| {
                preferences.model.as_deref() != Some(settings.model.as_str())
                    || preferences.thinking_level != settings.thinking
            });
        (Some(settings), correct_preferences)
    } else {
        (None, false)
    };
    let initial_model = initial_settings
        .as_ref()
        .map_or_else(|| model, |settings| settings.model.clone());
    let initial_thinking = initial_settings
        .as_ref()
        .map_or(default_thinking, |settings| settings.thinking);
    let mut terminal_guard =
        TerminalGuard::enter(options.appearance.terminal_background(options.color_mode))?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let runtime = Runtime::new(512);
    let mut pending_session_bootstrap = Vec::new();
    let mut state = TuiState::with_options(cwd.clone(), initial_model, profile, options);
    state.thinking = initial_thinking;
    state.model_details = initial_models
        .into_iter()
        .map(|model| (model.id.clone(), model))
        .collect();
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
        if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
            store.append_all(&created)?;
        } else {
            pending_session_bootstrap.extend(created.iter().cloned());
        }
        runner.restore_session(&session_id, &created).await?;
        for event in created {
            state.apply(&event.event);
        }
        let initial_settings = initial_settings
            .as_ref()
            .expect("new TUI sessions reconcile catalog settings before creation");
        if store.as_ref().is_some_and(SessionStore::is_active) {
            runner
                .set_model_settings(&session_id, initial_settings)
                .await?;
        }
        let selected_settings = runtime
            .dispatch(AppCommand::ChangeModelSettings {
                session_id: session_id.clone(),
                model: state.model.clone(),
                thinking: state.thinking,
                reset_security: false,
            })
            .await?;
        if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
            if correct_initial_preferences {
                store.append_all_and_set_profile_preferences(
                    &selected_settings,
                    &state.model,
                    state.thinking,
                )?;
            } else {
                store.append_all(&selected_settings)?;
            }
        } else {
            pending_session_bootstrap.extend(selected_settings.iter().cloned());
        }
        for event in selected_settings {
            state.apply(&event.event);
        }
        state.status = "Ready".into();
        state.status_tone = StatusTone::Success;
        session_id
    };

    let mut input_events = EventStream::new();
    let (ui_tx, mut ui_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    if !uses_deterministic_test_runner() {
        let update_tx = ui_tx.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(Duration::from_secs(6 * 60 * 60));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut previous = None;
            loop {
                tokio::select! {
                    () = update_tx.closed() => break,
                    _ = ticks.tick() => {
                        if let Ok(Some(notice)) = crate::updates::startup_notice().await
                            && previous.as_ref() != Some(&notice) {
                            previous = Some(notice.clone());
                            if update_tx.send(UiMessage::UpdateAvailable(notice)).await.is_err() { break; }
                        }
                    }
                }
            }
        });
    }
    let mut billing_flow = BillingFlow::new(&auth)?;
    let mut billing_ticks = tokio::time::interval(Duration::from_secs(5));
    billing_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cancellation: Option<CancellationToken> = None;
    let title_lifetime = CancellationToken::new();
    let _title_cancel_on_exit = title_lifetime.clone().drop_guard();
    let mut title_tasks = tokio::task::JoinSet::new();
    let mut auth_cancellation: Option<CancellationToken> = None;
    let mut security_cancellation: Option<CancellationToken> = None;
    let mut security_generation = 0_u64;
    let mut startup_auth_pending = false;
    let mut active_turn: Option<TurnId> = None;
    let mut pending_approval: Option<(ApprovalRequest, oneshot::Sender<ApprovalResponse>)> = None;
    let mut active_steering: Option<Arc<crate::steering::TurnSteering>> = None;
    let mut pending_questions: Option<PendingQuestions> = None;
    let mut animation_ticks = tokio::time::interval(ANIMATION_TICK);
    animation_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Renewal and inactivity must advance even with animations disabled.
    let mut accounting_ticks = tokio::time::interval(Duration::from_secs(5));
    accounting_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut accounting_busy = false;
    let mut accounting_attempts: BTreeMap<String, u16> = BTreeMap::new();
    let mut shutdown_signal = Box::pin(wait_for_shutdown_signal());
    let mut account_transition_error = None;

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

    let mut update_requested = false;
    'ui: loop {
        if state.running || state.compacting {
            state.proof_refresh.record_activity(Instant::now());
        }
        terminal.draw(|frame| render(frame, &state))?;
        tokio::select! {
            _ = billing_ticks.tick(), if matches!(&state.overlay, Some(Overlay::Billing(view)) if !view.busy && !view.redeem) => {
                billing_flow.start(&mut state, None, &ui_tx);
            }
            _ = accounting_ticks.tick(), if !accounting_busy && store.as_ref().is_some_and(SessionStore::is_active) => {
                accounting_attempts.retain(|id, _| state.request_usage.get(id).is_some_and(|r| !r.settled));
                let ids: Vec<_> = state.request_usage.values().filter(|r|
                    !r.settled && r.state != axiom_inference::InvocationState::Running
                    && accounting_attempts.get(&r.request_id).copied().unwrap_or(0) < 240)
                    .take(100).map(|r| r.request_id.clone()).collect();
                if !ids.is_empty() {
                    for id in &ids { *accounting_attempts.entry(id.clone()).or_default() += 1; }
                    accounting_busy = true;
                    let task_runner = Arc::clone(&runner);
                    let tx = ui_tx.clone();
                    let owner = session_id.clone();
                    let account_generation = auth.generation();
                    tokio::spawn(async move {
                        let result = tokio::time::timeout(Duration::from_secs(5),
                            task_runner.recover_accounting(&ids, CancellationToken::new())).await
                            .unwrap_or(Err(AxiomError::Cancelled));
                        let _ = tx.send(UiMessage::AccountingRecovered { session_id: owner, account_generation, result }).await;
                    });
                }
            }
            () = &mut shutdown_signal => {
                if let Some(token) = &cancellation {
                    token.cancel();
                }
                if let Some(token) = auth_cancellation.take() {
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
                    if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
                        store.append_all(&cancelled)?;
                    }
                }
                break;
            }
            _ = animation_ticks.tick(), if state.should_animate() => {
                state.tick_animation();
            }
            maybe_event = input_events.next() => {
                let Some(Ok(event)) = maybe_event else { break };
                if matches!(&event, Event::Key(key) if key.kind != KeyEventKind::Release)
                    || matches!(&event, Event::Paste(_) | Event::Mouse(_) | Event::FocusGained)
                {
                    state.proof_refresh.record_activity(Instant::now());
                }
                let composer_before = state.input.to_string();
                match event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if key.code == KeyCode::Esc
                            && (state.interaction.is_some()
                                || (state.overlay.is_none() && !state.search_active))
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
                                    if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
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
                            if let Some(choice) = approval_choice(key, request) {
                                let (request, response) = pending_approval.take().expect("pending approval exists");
                                let _ = response.send(ApprovalResponse { choice });
                                state.interaction = None;
                                let decision = match choice {
                                    ApprovalChoice::AllowOnce => "Allowed once",
                                    ApprovalChoice::AllowExactSession => "Allowed exact operation for session",
                                    ApprovalChoice::AllowPrefixSession => "Allowed safe scope for session",
                                    ApprovalChoice::Deny => "Denied",
                                };
                                state.status = format!("{decision} · {}", request.request_id);
                            } else {
                                state.status = approval_status(request);
                            }
                        } else if let Some(Overlay::Billing(view)) = &mut state.overlay {
                            state.exit_armed = false;
                            let action = view.key(key);
                            match key.code {
                                KeyCode::Up => state.overlay_scroll = state.overlay_scroll.saturating_sub(1),
                                KeyCode::Down => state.overlay_scroll = state.overlay_scroll.saturating_add(1),
                                KeyCode::PageUp => state.overlay_scroll = state.overlay_scroll.saturating_sub(10),
                                KeyCode::PageDown => state.overlay_scroll = state.overlay_scroll.saturating_add(10),
                                _ => {}
                            }
                            match action {
                                BillingAction::None => {},
                                BillingAction::Close => { billing_flow.cancel(); state.overlay = None; state.overlay_scroll = 0; },
                                BillingAction::Refresh => billing_flow.start(&mut state, None, &ui_tx),
                                BillingAction::Redeem(code) => billing_flow.start(&mut state, Some(code), &ui_tx),
                                BillingAction::CopyAddress => { BillingFlow::copy_address(&mut state)?; },
                            }
                        } else if matches!(state.overlay, Some(Overlay::Auth(_))) {
                            state.exit_armed = false;
                            match &state.overlay {
                                Some(Overlay::Auth(AuthOverlay::Menu { .. })) => match key.code {
                                    KeyCode::Esc => state.overlay = None,
                                    KeyCode::Enter => {
                                        state.overlay = Some(Overlay::Auth(AuthOverlay::Starting));
                                        state.status = "Opening secure browser authorization…".into();
                                        let token = CancellationToken::new();
                                        auth_cancellation = Some(token.clone());
                                        let login_auth = auth.clone();
                                        let login_tx = ui_tx.clone();
                                        tokio::spawn(async move {
                                            let result = tokio::select! {
                                                () = token.cancelled() => Err(AxiomError::Cancelled),
                                                result = login_auth.start_native_login(None) => result,
                                            };
                                            let _ = login_tx
                                                .send(UiMessage::NativeLoginStarted(Box::new(result)))
                                                .await;
                                        });
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
                                                if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
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
                                        let model_changed = model != state.model;
                                        state.overlay = None;
                                        state.status = format!("Switching to {model}…");
                                        let selection = match runner
                                            .available_model_details(CancellationToken::new())
                                            .await
                                        {
                                            Ok(models) => models
                                                .iter()
                                                .find(|candidate| candidate.id == model)
                                                .map(|candidate| {
                                                    reconcile_model_settings(
                                                        candidate,
                                                        state.thinking,
                                                    )
                                                })
                                                .ok_or_else(|| {
                                                    AxiomError::InvalidTransition(
                                                        "model is no longer in the provider catalog"
                                                            .into(),
                                                    )
                                                }),
                                            Err(error) => Err(error),
                                        };
                                        match selection {
                                            Ok(selection) => match runner
                                                .set_model_settings(&session_id, &selection)
                                                .await
                                            {
                                            Ok(()) => match runtime
                                                .dispatch(AppCommand::ChangeModelSettings {
                                                    session_id: session_id.clone(),
                                                    model: selection.model.clone(),
                                                    thinking: selection.thinking,
                                                    reset_security: model_changed,
                                                })
                                                .await
                                            {
                                                Ok(events) => {
                                                    if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
                                                        store
                                                            .append_all_and_set_profile_preferences(
                                                                &events,
                                                                &selection.model,
                                                                selection.thinking,
                                                            )?;
                                                    }
                                                    for event in events {
                                                        state.apply(&event.event);
                                                    }
                                                    if let Some(token) = security_cancellation.take() { token.cancel(); }
                                                    security_generation = security_generation.wrapping_add(1);
                                                }
                                                Err(error) => show_slash_error(
                                                    &mut state,
                                                    &error.to_string(),
                                                ),
                                            },
                                            Err(error) => {
                                                show_slash_error(&mut state, &error.to_string());
                                            }
                                        },
                                            Err(error) => show_slash_error(
                                                &mut state,
                                                &error.to_string(),
                                            ),
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
                                            if let Some(token) = security_cancellation.take() { token.cancel(); }
                                                    security_generation = security_generation.wrapping_add(1);
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
                                    if state.running {
                                        show_slash_error(&mut state, "Stop the active task before refreshing attestation.");
                                    } else if auth.has_credential() {
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
                                KeyCode::Enter if state.running && state.focus == Focus::Composer
                                    && !state.input.trim().is_empty() && slash::parse(&state.input).is_none() => {
                                    if let (Some(inbox), Some(turn)) = (&active_steering, &active_turn) {
                                        let text = state.input.to_string();
                                        match inbox.submit(turn, uuid::Uuid::new_v4().to_string(), text.clone()) {
                                            Ok(receiver) => {
                                                state.input.take_text();
                                                state.status = "Steering queued · Waiting for current step to finish…".into();
                                                let tx = ui_tx.clone(); let session_id = session_id.clone();
                                                tokio::spawn(async move {
                                                    let applied = receiver.await.is_ok();
                                                    let _ = tx.send(UiMessage::SteeringDone { session_id, text, applied }).await;
                                                });
                                            }
                                            Err(error) => show_slash_error(&mut state, &error.to_string()),
                                        }
                                    }
                                }
                                KeyCode::Enter
                                    if state.focus == Focus::Composer && state.should_accept_slash_selection() =>
                                {
                                    state.accept_slash_selection();
                                }
                                KeyCode::Enter if state.focus == Focus::Composer && !state.input.trim().is_empty() => {
                                    let prompt = state.input.take_text();
                                    state.reset_slash_completion();
                                    if let Some(command) = slash::parse(&prompt) {
                                        if state.running && command.as_ref().is_ok_and(|command| !command_allowed_while_running(command)) {
                                            state.input.set_text(prompt);
                                            show_slash_error(&mut state, "Stop the active task before using this command. Your draft is still here.");
                                            continue;
                                        }
                                        match command {
                                            Err(error) => show_slash_error(&mut state, &error.to_string()),
                                            Ok(SlashCommand::Update) => {
                                                update_requested = true;
                                                break 'ui;
                                            }
                                            Ok(SlashCommand::Help) => {
                                                state.push_entry(
                                                    EntryKind::System,
                                                    slash_help_text(),
                                                    false,
                                                );
                                                state.status = "Slash commands".into();
                                            }
                                            Ok(SlashCommand::Usage) => {
                                                state.overlay = Some(Overlay::Usage);
                                                state.overlay_scroll = 0;
                                            }
                                            Ok(SlashCommand::Theme(appearance)) => {
                                                state.options.appearance = appearance;
                                                let background = appearance.terminal_background(state.options.color_mode);
                                                if background.is_some() {
                                                    terminal_guard.reset_background = true;
                                                    execute!(io::stdout(), SetAxiomTerminalBackground(background))?;
                                                } else if terminal_guard.reset_background {
                                                    execute!(io::stdout(), ResetAxiomTerminalBackground(true))?;
                                                    terminal_guard.reset_background = false;
                                                }
                                                state.status = format!("{} appearance · /theme to change", appearance.label());
                                                state.status_tone = StatusTone::Neutral;
                                            }
                                            Ok(SlashCommand::Web(enabled)) => {
                                                state.web_enabled = enabled;
                                                state.status = if enabled {
                                                    "Web on · queries and fetched URLs are not end-to-end encrypted"
                                                } else {
                                                    "Web off · search and URL fetching disabled"
                                                }.into();
                                                state.status_tone = StatusTone::Neutral;
                                                if enabled {
                                                    state.push_entry(EntryKind::System,
                                                        "Web enabled for subsequent requests. Search queries and fetched URLs are shared with external services; they are not end-to-end encrypted. Model traffic remains encrypted. Use /web off to disable.".into(), false);
                                                }
                                            }
                                            Ok(SlashCommand::Permissions) => {
                                                state.open_permissions();
                                            }
                                            Ok(SlashCommand::Model) => {
                                                state.open_model_picker();
                                                if !state.model_catalog_loading
                                                {
                                                    state.model_catalog_loading = true;
                                                    state.status =
                                                        "Loading available models…".into();
                                                    let task_runner = runner.clone();
                                                    let task_tx = ui_tx.clone();
                                                    tokio::spawn(async move {
                                                        let result = task_runner
                                                            .available_model_details(
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
                                                match store.as_ref().filter(|store| store.is_active()) {
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
                                                match store.as_ref().filter(|store| store.is_active()) {
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
                                                let selection = runner.available_model_details(CancellationToken::new()).await.and_then(|models| {
                                                    let model = models.iter().find(|model| model.id == state.model)
                                                        .ok_or_else(|| AxiomError::Config("current model is not in the provider catalog".into()))?;
                                                    Ok(crate::agent::reconcile_model_settings(model, level).thinking)
                                                });
                                                let level = match selection {
                                                    Ok(level) => level,
                                                    Err(error) => { show_slash_error(&mut state, &error.to_string()); continue; }
                                                };
                                                match runner.set_thinking_level(&session_id, level).await {
                                                    Ok(()) => match runtime.dispatch(AppCommand::ChangeThinkingLevel {
                                                        session_id: session_id.clone(),
                                                        level,
                                                    }).await {
                                                        Ok(events) => {
                                                            persist_tui_thinking_change(
                                                                store.as_ref(),
                                                                &events,
                                                                &state.model,
                                                                level,
                                                            )?;
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
                                            Ok(command @ (SlashCommand::Balance | SlashCommand::Topup | SlashCommand::Redeem)) => {
                                                billing_flow.cancel();
                                                let redeem = command == SlashCommand::Redeem;
                                                state.overlay = Some(Overlay::Billing(Box::new(BillingView::new(redeem))));
                                                state.overlay_scroll = 0;
                                                if !redeem { billing_flow.start(&mut state, None, &ui_tx); }
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
                                                state.overlay = Some(Overlay::Auth(
                                                    AuthOverlay::Checking {
                                                        label: "Signing out of Axiom…",
                                                    },
                                                ));
                                                state.status = "Removing saved Axiom authorization…".into();
                                                state.status_tone = StatusTone::Waiting;
                                                let logout_auth = auth.clone();
                                                let logout_tx = ui_tx.clone();
                                                tokio::spawn(async move {
                                                    let result = logout_auth.logout_async().await;
                                                    let _ = logout_tx
                                                        .send(UiMessage::AuthLoggedOut(result))
                                                        .await;
                                                });
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
                                            Ok(SlashCommand::AcceptOutdatedTee) => {
                                                if auth.has_credential() {
                                                    start_security_check(&mut state, &runner, &ui_tx, &mut security_cancellation, &mut security_generation, SecurityCheck::AcceptOutdated);
                                                    state.status = "Checking TEE with your outdated-provider consent…".into();
                                                    state.status_tone = StatusTone::Waiting;
                                                } else { state.open_login(Some("Sign in before accepting a provider warning.".into())); }
                                            }
                                            Ok(SlashCommand::Security) => {
                                                state.open_security();
                                                if !state.running
                                                    && state.report_status() != super::attestation::ReportStatus::Verified
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
                                    if !uses_deterministic_test_runner()
                                        && !store.as_ref().is_some_and(SessionStore::is_active)
                                    {
                                        state.input.set_text(prompt);
                                        state.open_login(Some(
                                            "Sign in before starting this task. Your draft is still here."
                                                .into(),
                                        ));
                                        continue;
                                    }
                                    let turn_id = runtime.turn_id();
                                    let token = CancellationToken::new();
                                    let should_generate_title = !state.has_prompt
                                        && store.as_ref().is_some_and(SessionStore::is_active);
                                    let submitted = runtime.dispatch(AppCommand::SubmitPrompt {attachments: Vec::new(),
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        text: prompt.clone(),
                                    }).await?;
                                    if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
                                        store.append_all(&submitted)?;
                                        store.set_last_used_model(&state.model)?;
                                        store.set_last_used_thinking(state.thinking)?;
                                        if should_generate_title {
                                            match crate::session_title::TitleGeneration::prepare(store, &session_id, &prompt, &state.model) {
                                                Ok(Some(job)) => {
                                                    let title_runner = runner.clone();
                                                    let title_token = token.child_token();
                                                    let lifetime = title_lifetime.clone();
                                                    let title_tx = ui_tx.clone();
                                                    let owner = session_id.clone();
                                                    let account_generation = auth.generation();
                                                    title_tasks.spawn(async move {
                                                        let request = job.run(title_runner.as_ref(), title_token.clone());
                                                        tokio::pin!(request);
                                                        let result = tokio::select! {
                                                            result = &mut request => result,
                                                            () = lifetime.cancelled() => { title_token.cancel(); request.await }
                                                        };
                                                        if let Err(error) = result { tracing::debug!(%error, "background title kept its local fallback"); }
                                                        tokio::select! {
                                                            _ = title_tx.send(UiMessage::TitleFinished { session_id: owner, account_generation }) => {},
                                                            () = lifetime.cancelled() => {},
                                                        }
                                                    });
                                                }
                                                Ok(None) => {},
                                                Err(error) => tracing::warn!(%session_id, %error, "could not persist session title"),
                                            }
                                        }
                                    }
                                    for event in submitted {
                                        state.apply(&event.event);
                                    }
                                    cancellation = Some(token.clone());
                                    active_turn = Some(turn_id.clone());
                                    let steering = Arc::new(crate::steering::TurnSteering::new(turn_id.clone()));
                                    active_steering = Some(steering.clone());
                                    let task_runner = runner.clone();
                                    let task_tx = ui_tx.clone();
                                    let context = TurnContext {attachments: Vec::new(),
                                        session_id: session_id.clone(),
                                        turn_id: turn_id.clone(),
                                        cwd: cwd.clone(),
                                        permission_profile: state.profile,
                                        web_enabled: state.web_enabled,
                                        steering: Some(steering),
                                        approval: Some(Arc::new(TuiApproval { tx: task_tx.clone() })),
                                        questions: Some(Arc::new(TuiQuestions { tx: task_tx.clone() })),
                                    };
                                    // Keep the warmup alive for the provider's singleflight wait,
                                    // but never let its late UI result overwrite this turn's evidence.
                                    let preflight_guard = security_cancellation.take().map(CancellationToken::drop_guard);
                                    security_generation = security_generation.wrapping_add(1);
                                    tokio::spawn(async move {
                                        let _preflight_guard = preflight_guard;
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
                                KeyCode::Char(character) if state.focus == Focus::Composer && is_text_input(key.modifiers) => {
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
                        state.paste_text(&text);
                    }
                    _ => {}
                }
                let draft = state.input.as_str();
                if draft != composer_before && !draft.trim().is_empty() && !draft.starts_with('/')
                    && state.focus == Focus::Composer && state.overlay.is_none() && state.interaction.is_none()
                    && !startup_auth_pending && auth.has_credential()
                    && store.as_ref().is_some_and(SessionStore::is_active)
                {
                    state.proof_refresh.enable();
                    if security_cancellation.is_none()
                        && state.security_refresh_due(Instant::now(), u64::try_from(chrono::Utc::now().timestamp()).unwrap_or(0))
                    {
                        start_security_check(&mut state, &runner, &ui_tx,
                            &mut security_cancellation, &mut security_generation, SecurityCheck::Prewarm);
                    }
                }
            }
            message = ui_rx.recv() => {
                let Some(message) = message else { break };
                match message {
                    UiMessage::UpdateAvailable(notice) => {
                        // UI-only information: never model input or persisted conversation history.
                        state.push_entry(EntryKind::System, notice, false);
                    }
                    UiMessage::TitleFinished { session_id: owner, account_generation } => {
                        if owner == session_id && account_generation == auth.generation()
                            && let Some(store) = store.as_ref().filter(|store| store.is_active())
                            && let Ok(snapshot) = store.thread_snapshot(&owner, None, 1)
                        {
                            for usage in snapshot.request_usage.into_iter().filter(|usage| usage.purpose == axiom_inference::InvocationPurpose::Title) {
                                state.apply(&AppEvent::RequestUsageUpdated { usage });
                            }
                            if let Some(Overlay::ResumePicker { sessions, .. } | Overlay::DeletePicker { sessions, .. }) = &mut state.overlay
                                && let Some(session) = sessions.iter_mut().find(|session| session.id == owner.to_string()) {
                                session.title = snapshot.thread.title;
                            }
                        }
                    }
                    UiMessage::BillingFinished { generation, account_generation, result } => {
                        billing_flow.finish(&mut state, generation, account_generation, result);
                        billing_ticks.reset();
                    }
                    UiMessage::AccountingRecovered { session_id: owner, account_generation, result } => {
                        accounting_busy = false;
                        if owner == session_id && account_generation == auth.generation()
                            && let Ok(records) = result
                            && let Some(store) = store.as_ref().filter(|s| s.is_active())
                        {
                            store.reconcile_request_usage(&owner, &records)?;
                            for usage in store.thread_snapshot(&owner, None, 1)?.request_usage {
                                state.apply(&AppEvent::RequestUsageUpdated { usage });
                            }
                        }
                    }
                    UiMessage::SteeringDone { session_id: owner, text, applied } => {
                        if owner == session_id && !applied {
                            let draft = state.input.take_text();
                            state.input.set_text(if draft.is_empty() { text } else { format!("{text}\n\n{draft}") });
                            state.status = "Steering was not applied · Message returned to composer".into();
                            state.status_tone = StatusTone::Waiting;
                        }
                    }
                    UiMessage::Event(event) => {
                        if matches!(event, AppEvent::SecurityStatusChanged { .. }) {
                            // Security is live process state, not resumable
                            // conversation state.
                            state.apply(&event);
                        } else {
                            let envelope = runtime.emit(session_id.clone(), event.clone()).await?;
                            if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
                                store.append(&envelope)?;
                            }
                            if let AppEvent::SteeringApplied { client_item_id, .. } = &event
                                && let Some(inbox) = &active_steering { inbox.acknowledge(client_item_id); }
                            state.apply(&event);
                        }
                    }
                    UiMessage::SecurityPreflightDone(generation, result)
                        if generation == security_generation =>
                    {
                        security_cancellation = None;
                        state.proof_refresh.completed(Instant::now(), matches!(&result,
                            Ok(SecurityVerification { status: crate::app::SecurityStatus::Verified | crate::app::SecurityStatus::Degraded, .. })
                        ));
                        match result {
                            Ok(SecurityVerification {
                                status:
                                    security @ (crate::app::SecurityStatus::Verified
                                    | crate::app::SecurityStatus::Degraded
                                    | crate::app::SecurityStatus::UnattestedDevelopment),
                                evidence,
                            }) => {
                                let manually_refreshed =
                                    state.status == "Refreshing attestation…";
                                state.security_evidence = evidence;
                                state.apply(&AppEvent::SecurityStatusChanged { state: security });
                                if security == crate::app::SecurityStatus::Degraded {
                                    state.status = "TEE security updates needed · Accepted for this provider until restart".into();
                                    state.status_tone = StatusTone::Waiting;
                                } else if manually_refreshed {
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
                                state.fail_security("Security verification returned no final verdict");
                                state.status =
                                    "Security verification returned no final verdict".into();
                                state.status_tone = StatusTone::Error;
                            }
                            Err(AxiomError::Cancelled) => { state.reset_security(); }
                            Err(AxiomError::SecureProvider { code: "PROVIDER_TDX_OUT_OF_DATE", .. }) => {
                                state.security_evidence = None;
                                state.apply(&AppEvent::SecurityStatusChanged { state: crate::app::SecurityStatus::Outdated });
                                state.status = "TEE security updates needed · To accept this provider until restart: /security accept-outdated".into();
                                state.status_tone = StatusTone::Waiting;
                            }
                            Err(error) => {
                                let detail = sanitize_terminal_text(&error.to_string());
                                state.fail_security(&detail);
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
                        if let Some(inbox) = active_steering.take() { inbox.close(); }
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
                        if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
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
                                        if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
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
                                models.retain(|model| {
                                    !model.id.trim().is_empty()
                                        && !model.id.chars().any(char::is_whitespace)
                                });
                                models.sort_by(|left, right| left.id.cmp(&right.id));
                                models.dedup_by(|left, right| left.id == right.id);
                                models.sort_by(crate::agent::compare_model_preference);
                                state.model_candidates =
                                    models.iter().map(|model| model.id.clone()).collect();
                                state.model_details = models
                                    .into_iter()
                                    .map(|model| (model.id.clone(), model))
                                    .collect();
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
                                state.model_details.clear();
                                state.status = format!(
                                    "Could not load available models: {error} · Esc to close"
                                );
                            }
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
                    UiMessage::NativeLoginStarted(result) => match *result {
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
                            Ok(account) if !auth.account_status_is_current(&account) => {}
                            Ok(account) => {
                                if let Some(account_store) = &store
                                    && let Err(error) = activate_tui_account_store(
                                        account_store,
                                        &account.account.id,
                                    )
                                {
                                    account_transition_error = Some(error);
                                    break 'ui;
                                }
                                if let Some(account_store) = &store
                                    && !pending_session_bootstrap.is_empty()
                                    && let Err(error) = finish_authenticated_bootstrap(
                                        &runtime, &runner, account_store, &session_id,
                                        &mut state, &mut pending_session_bootstrap,
                                    ).await
                                {
                                    let _ = account_store.deactivate_account();
                                    account_transition_error = Some(error);
                                    break 'ui;
                                }
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
                                // Composing starts verification; opening a thread/account does not.
                            }
                            Err(AxiomError::Cancelled) => {}
                            Err(error) => state.open_login(Some(error.to_string())),
                        }
                    }
                    UiMessage::AuthLoggedOut(result) => match result {
                        Ok(()) => {
                            if let Some(account_store) = &store {
                                let _ = account_store.deactivate_account();
                            }
                            state.overlay = None;
                            state.reset_security();
                            state.status = "Signed out of Axiom".into();
                            state.status_tone = StatusTone::Neutral;
                            state.push_entry(
                                EntryKind::System,
                                "AXIOM ACCOUNT\nSigned out. The native refresh token was removed from this computer."
                                    .into(),
                                false,
                            );
                            // The runtime projection still contains the old
                            // account's transcript. End this TUI instance so
                            // it can never be relabeled or persisted after a
                            // subsequent account signs in.
                            break 'ui;
                        }
                        Err(error) => {
                            state.overlay = None;
                            show_slash_error(&mut state, &error.to_string());
                        }
                    },
                    UiMessage::AuthValidated(purpose, status) => {
                        if purpose == ValidationPurpose::Startup {
                            startup_auth_pending = false;
                        }
                        if matches!(
                            &status,
                            ValidationStatus::Valid(account)
                                if !auth.account_status_is_current(account)
                        ) {
                            continue;
                        }
                        match (purpose, status) {
                            (_, ValidationStatus::Valid(account)) => {
                                if let Some(account_store) = &store
                                    && let Err(error) = activate_tui_account_store(
                                        account_store,
                                        &account.account.id,
                                    )
                                {
                                    account_transition_error = Some(error);
                                    break 'ui;
                                }
                                if let Some(account_store) = &store
                                    && !pending_session_bootstrap.is_empty()
                                    && let Err(error) = finish_authenticated_bootstrap(
                                        &runtime, &runner, account_store, &session_id,
                                        &mut state, &mut pending_session_bootstrap,
                                    ).await
                                {
                                    let _ = account_store.deactivate_account();
                                    account_transition_error = Some(error);
                                    break 'ui;
                                }
                                let detail = account_status_text(&account);
                                if purpose == ValidationPurpose::Startup {
                                    state.status = "Ready".into();
                                    state.status_tone = StatusTone::Success;
                                    // Composing starts verification; opening a thread/account does not.
                                } else {
                                    if purpose == ValidationPurpose::Login {
                                        state.reset_security();
                                    }
                                    state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                        status: if purpose == ValidationPurpose::Login {
                                            format!("Axiom account connected.\n\n{detail}")
                                        } else {
                                            format!("Connected to Axiom.\n\n{detail}")
                                        },
                                    }));
                                    state.status = "Ready".into();
                                    state.status_tone = StatusTone::Success;
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
                            (ValidationPurpose::Account, ValidationStatus::Expired) => {
                                let had_account = store
                                    .as_ref()
                                    .is_some_and(SessionStore::is_active);
                                if let Some(account_store) = &store {
                                    let _ = account_store.deactivate_account();
                                }
                                state.reset_security();
                                state.overlay = Some(Overlay::Auth(AuthOverlay::Account {
                                    status:
                                        "The saved Axiom account session expired or was revoked. Use /login to reconnect."
                                            .into(),
                                }));
                                state.status = "Axiom account session expired".into();
                                state.status_tone = StatusTone::Error;
                                if had_account {
                                    break 'ui;
                                }
                            }
                            (_, ValidationStatus::Expired) => {
                                let had_account = store
                                    .as_ref()
                                    .is_some_and(SessionStore::is_active);
                                if let Some(account_store) = &store {
                                    let _ = account_store.deactivate_account();
                                }
                                state.reset_security();
                                state.open_login(Some(
                                    "The saved Axiom account session expired or was revoked. Sign in again."
                                        .into(),
                                ));
                                if had_account {
                                    break 'ui;
                                }
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
                                        "The account session was saved, but Axiom could not be reached to validate it.\n\n{message}"
                                    ),
                                }));
                                state.status = "Account session saved · validation unavailable".into();
                                state.status_tone = StatusTone::Waiting;
                            }
                            (ValidationPurpose::Startup, ValidationStatus::Unavailable(message)) => {
                                state.status = format!("Login validation unavailable: {message}");
                                state.status_tone = StatusTone::Waiting;
                            }
                        }
                    }
                }
            }
        }
    }

    title_lifetime.cancel();
    while title_tasks.join_next().await.is_some() {}
    if let Some(token) = security_cancellation.take() {
        token.cancel();
    }
    let closed = runtime
        .dispatch(AppCommand::CloseSession {
            session_id: session_id.clone(),
        })
        .await?;
    if let Some(store) = store.as_ref().filter(|store| store.is_active()) {
        store.append_all(&closed)?;
    }

    if let Some(error) = account_transition_error {
        return Err(error);
    }

    if update_requested {
        drop(terminal);
        drop(terminal_guard);
        drop(store);
        crate::updates::run_with_resume(cwd, session_id.to_string())
            .await
            .map_err(|error| crate::AxiomError::Config(error.to_string()))?;
    }
    Ok(())
}
