use std::{path::PathBuf, str::FromStr as _, sync::Arc};

use anyhow::Context as _;
#[cfg(feature = "development-provider")]
use axiomcli::provider::AxiomProvider;
use axiomcli::{
    AxiomError,
    agent::{
        APP_EVENT_QUEUE_CAPACITY, AgentEngine, AgentLimits, ApprovalTurnRunner, BlockingTurnRunner,
        EchoTurnRunner, PlanReviewTurnRunner, QuestionTurnRunner, TurnContext, TurnRunner,
        WorkspaceEditTurnRunner,
    },
    app::{AppCommand, AppEvent, Origin, Runtime, SessionId},
    auth::{AuthManager, ValidationStatus},
    config::Config,
    planning::PlanArtifact,
    policy::{EffectClass, PolicyRule, RuleAction},
    provider::{InferenceProvider, SecureAxiomProvider},
    session::{ExportOptions, RetentionPolicy, SessionStore},
    tools::default_registry,
};
use clap::{Parser, Subcommand};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(name = "axiomcli", version, about = "Early AxiomAI coding-agent TUI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the terminal interface in the current terminal viewport.
    Tui {
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Resume a durable session by ID. Its original workspace identity is enforced.
        #[arg(long)]
        resume: Option<String>,
        /// Run with deterministic read-only plan policy; only plan artifacts may be written.
        #[arg(long)]
        plan: bool,
    },
    /// Run one prompt without a terminal interface.
    Exec {
        prompt: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Run with deterministic read-only plan policy; only plan artifacts may be written.
        #[arg(long)]
        plan: bool,
    },
    /// Serve Agent Client Protocol over stdin/stdout.
    Acp,
    /// Check configuration and local dependencies.
    Doctor,
    /// List models exposed by the configured secure Axiom service.
    Models,
    /// Inspect and manage durable local session journals.
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
    /// Inspect and review persisted workspace plans.
    Plans {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[command(subcommand)]
        action: PlanAction,
    },
    /// Add restrictive, reviewable policy rules to a project's configuration.
    Permissions {
        #[command(subcommand)]
        action: PermissionAction,
    },
}

#[derive(Debug, Subcommand)]
enum PermissionAction {
    /// Persist an `ask` or `deny` rule; project rules can never grant authority.
    AddProjectRule {
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long, value_enum)]
        action: RuleAction,
        #[arg(long, value_enum)]
        effect: Option<EffectClass>,
        #[arg(long)]
        resource_prefix: Option<String>,
        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum PlanAction {
    List,
    Show {
        id: String,
    },
    Comment {
        id: String,
        start_line: usize,
        end_line: usize,
        text: String,
    },
    RequestRevision {
        id: String,
        start_line: usize,
        end_line: usize,
        text: String,
    },
    Revise {
        id: String,
        expected_revision: u64,
        markdown: String,
    },
    Propose {
        id: String,
    },
    Approve {
        id: String,
    },
    Abandon {
        id: String,
    },
}

#[derive(Debug, Subcommand)]
enum SessionAction {
    List {
        #[arg(long)]
        all: bool,
    },
    Show {
        id: String,
    },
    Export {
        id: String,
        #[arg(long)]
        omit_prompts: bool,
        #[arg(long)]
        omit_tool_output: bool,
        #[arg(long, default_value_t = 10_000)]
        max_events: usize,
    },
    Rename {
        id: String,
        title: String,
    },
    Archive {
        id: String,
    },
    Unarchive {
        id: String,
    },
    /// Apply explicit local journal retention limits.
    Prune {
        #[arg(long, default_value_t = 10_000)]
        max_events_per_session: usize,
        /// Permanently delete archived sessions older than this many days.
        #[arg(long)]
        delete_archived_older_than_days: Option<i64>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load().context("failed to load AxiomCLI configuration")?;
    let auth = AuthManager::new(&config.base_url, config.request_timeout())
        .context("failed to initialize Axiom authentication")?;
    init_tracing(matches!(cli.command, Some(Command::Acp)));

    match cli.command.unwrap_or(Command::Tui {
        cwd: None,
        resume: None,
        plan: false,
    }) {
        Command::Doctor => {
            doctor(&config, &auth).await;
            Ok(())
        }
        Command::Models => {
            let provider = build_provider(&config, &auth)?;
            for model in provider.models(CancellationToken::new()).await? {
                println!("{}", model.id);
            }
            Ok(())
        }
        Command::Sessions { action } => manage_sessions(action),
        Command::Plans { cwd, action } => manage_plans(&resolve_cwd(cwd)?, action),
        Command::Permissions { action } => manage_permissions(action),
        Command::Tui { cwd, resume, plan } => {
            let store = open_store()?;
            let resume = resume
                .map(|id| SessionId::from_str(&id).context("invalid session ID"))
                .transpose()?;
            let (cwd, saved_profile) = if let Some(id) = &resume {
                let summary = store.summary(id)?;
                let saved = summary.cwd.canonicalize().with_context(|| {
                    format!("saved workspace is unavailable: {}", summary.cwd.display())
                })?;
                if let Some(requested) = cwd {
                    let requested = resolve_cwd(Some(requested))?;
                    anyhow::ensure!(
                        requested == saved,
                        "--cwd does not match the resumed session workspace"
                    );
                }
                let profile = axiomcli::app::PermissionProfile::from_str(&summary.profile)?;
                anyhow::ensure!(
                    !plan || profile == axiomcli::app::PermissionProfile::Plan,
                    "--plan cannot change the permission profile of a resumed session"
                );
                (saved, Some(profile))
            } else {
                (resolve_cwd(cwd)?, None)
            };
            let workspace_config = config
                .for_workspace(&cwd)
                .context("failed to load workspace configuration")?;
            let profile = saved_profile.unwrap_or(if plan {
                axiomcli::app::PermissionProfile::Plan
            } else {
                workspace_config.permission_profile
            });
            axiomcli::tui::run(
                build_runner(&workspace_config, &auth).await?,
                cwd,
                workspace_config.model.clone(),
                profile,
                axiomcli::tui::TuiLaunch {
                    options: axiomcli::tui::TuiOptions::from_environment(
                        workspace_config.animation,
                    ),
                    store: Some(store),
                    resume,
                    auth,
                },
            )
            .await
            .map_err(anyhow::Error::from)
        }
        Command::Exec { prompt, cwd, plan } => {
            let cwd = resolve_cwd(cwd)?;
            let workspace_config = config
                .for_workspace(&cwd)
                .context("failed to load workspace configuration")?;
            let profile = if plan {
                axiomcli::app::PermissionProfile::Plan
            } else {
                workspace_config.permission_profile
            };
            run_headless(
                build_runner(&workspace_config, &auth).await?,
                cwd,
                profile,
                workspace_config.model.clone(),
                prompt,
                open_store()?,
            )
            .await
        }
        Command::Acp => axiomcli::acp::serve_stdio(
            build_runner(&config, &auth).await?,
            config.clone(),
            Some(open_store()?),
            auth,
        )
        .await
        .map_err(anyhow::Error::from),
    }
}

fn manage_plans(cwd: &std::path::Path, action: PlanAction) -> anyhow::Result<()> {
    match action {
        PlanAction::List => {
            for plan in PlanArtifact::list(cwd)? {
                println!("{}\tr{}\t{:?}", plan.id, plan.revision, plan.state);
            }
        }
        PlanAction::Show { id } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&PlanArtifact::load(cwd, &id)?)?
            );
        }
        PlanAction::Comment {
            id,
            start_line,
            end_line,
            text,
        } => {
            let mut plan = PlanArtifact::load(cwd, &id)?;
            plan.comment(start_line, end_line, text)?;
            plan.save(cwd)?;
        }
        PlanAction::RequestRevision {
            id,
            start_line,
            end_line,
            text,
        } => {
            let mut plan = PlanArtifact::load(cwd, &id)?;
            plan.request_revision(start_line, end_line, text)?;
            plan.save(cwd)?;
        }
        PlanAction::Revise {
            id,
            expected_revision,
            markdown,
        } => {
            let mut plan = PlanArtifact::load(cwd, &id)?;
            plan.revise(expected_revision, markdown)?;
            plan.save(cwd)?;
        }
        PlanAction::Propose { id } => {
            let mut plan = PlanArtifact::load(cwd, &id)?;
            plan.propose()?;
            plan.save(cwd)?;
        }
        PlanAction::Approve { id } => {
            let mut plan = PlanArtifact::load(cwd, &id)?;
            plan.approve()?;
            plan.save(cwd)?;
        }
        PlanAction::Abandon { id } => {
            let mut plan = PlanArtifact::load(cwd, &id)?;
            plan.abandon()?;
            plan.save(cwd)?;
        }
    }
    Ok(())
}

fn init_tracing(acp_mode: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if acp_mode {
        builder.with_writer(std::io::stderr).init();
    } else {
        builder.init();
    }
}

async fn doctor(config: &Config, auth: &AuthManager) {
    println!("AxiomCLI diagnostics");
    println!("model: {}", config.model);
    println!("base URL: {}", config.base_url);
    println!("security: local attestation and E2EE required per inference request");
    println!("permission profile: {}", config.permission_profile);
    println!("SearXNG: {}", config.searxng_url);
    let auth_status = match auth.validate().await {
        ValidationStatus::Missing => "not signed in".to_owned(),
        ValidationStatus::Valid(account) => format!(
            "valid ({}, stored in {})",
            account.key_name,
            account.source.label()
        ),
        ValidationStatus::Invalid => "invalid or revoked".to_owned(),
        ValidationStatus::Unavailable(message) => format!("not checked ({message})"),
    };
    println!("Axiom authorization: {auth_status}");
    let endpoint = format!("{}/", config.searxng_url.trim_end_matches('/'));
    let status = reqwest::Client::new()
        .get(endpoint)
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .map_or_else(
            |error| format!("unavailable ({error})"),
            |response| response.status().to_string(),
        );
    println!("SearXNG health: {status}");
    if let Some(path) = Config::user_path() {
        println!("user config: {}", path.display());
    }
    if let Some(path) = session_path() {
        println!("session database: {}", path.display());
    }
}

fn manage_permissions(action: PermissionAction) -> anyhow::Result<()> {
    match action {
        PermissionAction::AddProjectRule {
            cwd,
            action,
            effect,
            resource_prefix,
            reason,
        } => {
            anyhow::ensure!(
                resource_prefix.is_none() || effect.is_some(),
                "--resource-prefix requires --effect"
            );
            let root = resolve_cwd(cwd)?;
            let path = Config::add_project_rule(
                &root,
                PolicyRule {
                    action,
                    effect,
                    resource_prefix,
                    reason,
                },
            )?;
            println!(
                "Added a restrictive project rule to {}. It can require approval or deny an operation; it cannot grant authority.",
                path.display()
            );
        }
    }
    Ok(())
}

fn session_path() -> Option<PathBuf> {
    Config::data_dir().map(|directory| directory.join("sessions.sqlite3"))
}

fn open_store() -> anyhow::Result<SessionStore> {
    let path = session_path().context("platform data directory is unavailable")?;
    SessionStore::open(path).map_err(Into::into)
}

fn manage_sessions(action: SessionAction) -> anyhow::Result<()> {
    let store = open_store()?;
    match action {
        SessionAction::List { all } => {
            for session in store.list(all)? {
                println!(
                    "{}\t{}\t{}\t{}",
                    session.id,
                    session.title.as_deref().unwrap_or("(untitled)"),
                    session.profile,
                    session.cwd.display()
                );
            }
        }
        SessionAction::Show { id } => {
            let id = SessionId::from_str(&id).context("invalid session ID")?;
            println!("{}", store.export(&id)?);
            let recovery = store.recovery_status(&id)?;
            if recovery.interrupted_turns > 0 {
                eprintln!(
                    "warning: {} turn(s) were interrupted and will not be replayed",
                    recovery.interrupted_turns
                );
            }
        }
        SessionAction::Export {
            id,
            omit_prompts,
            omit_tool_output,
            max_events,
        } => {
            let id = SessionId::from_str(&id).context("invalid session ID")?;
            println!(
                "{}",
                store.export_with_options(
                    &id,
                    ExportOptions {
                        include_prompts: !omit_prompts,
                        include_tool_output: !omit_tool_output,
                        max_events,
                    },
                )?
            );
        }
        SessionAction::Rename { id, title } => {
            store.rename(
                &SessionId::from_str(&id).context("invalid session ID")?,
                &title,
            )?;
        }
        SessionAction::Archive { id } => {
            store.archive(
                &SessionId::from_str(&id).context("invalid session ID")?,
                true,
            )?;
        }
        SessionAction::Unarchive { id } => {
            store.archive(
                &SessionId::from_str(&id).context("invalid session ID")?,
                false,
            )?;
        }
        SessionAction::Prune {
            max_events_per_session,
            delete_archived_older_than_days,
        } => {
            let report = store.apply_retention(RetentionPolicy {
                max_events_per_session,
                delete_archived_older_than_days,
            })?;
            println!(
                "deleted {} archived session(s) and {} old event(s)",
                report.deleted_sessions, report.deleted_events
            );
        }
    }
    Ok(())
}

async fn build_runner(config: &Config, auth: &AuthManager) -> anyhow::Result<Arc<dyn TurnRunner>> {
    if cfg!(debug_assertions) {
        match std::env::var("AXIOMCLI_TEST_RUNNER").as_deref() {
            Ok("echo") => return Ok(Arc::new(EchoTurnRunner)),
            Ok("blocking") => return Ok(Arc::new(BlockingTurnRunner)),
            Ok("questions") => return Ok(Arc::new(QuestionTurnRunner)),
            Ok("approval") => return Ok(Arc::new(ApprovalTurnRunner)),
            Ok("workspace_edit") => return Ok(Arc::new(WorkspaceEditTurnRunner)),
            Ok("plan_review") => return Ok(Arc::new(PlanReviewTurnRunner)),
            _ => {}
        }
    }
    let provider = build_provider(config, auth)?;
    let mut tools = default_registry(config.max_tool_output_bytes, &config.searxng_url)?;
    for tool in
        axiomcli::mcp::connect_tools(&config.mcp_servers, config.max_tool_output_bytes).await?
    {
        tools.register(tool)?;
    }
    let policy_rules = config.policy_rules.clone();
    Ok(Arc::new(
        AgentEngine::with_limits(
            provider,
            Arc::new(tools),
            config.model.clone(),
            AgentLimits {
                max_steps: config.max_agent_steps,
                max_context_bytes: config.max_context_bytes,
                max_context_tokens: config.max_context_tokens,
                max_tool_output_bytes: config.max_tool_output_bytes,
                max_wall_time: config.max_turn_secs.map(std::time::Duration::from_secs),
            },
        )
        .with_policy_rules(policy_rules),
    ))
}

fn build_provider(
    config: &Config,
    auth: &AuthManager,
) -> anyhow::Result<Arc<dyn InferenceProvider>> {
    #[cfg(feature = "development-provider")]
    if std::env::var("AXIOMCLI_UNATTESTED_DEV").as_deref() == Ok("1") {
        return Ok(Arc::new(AxiomProvider::from_environment(
            &config.base_url,
            config.request_timeout(),
        )?));
    }
    Ok(Arc::new(SecureAxiomProvider::with_auth(
        &config.base_url,
        auth.clone(),
        config.request_timeout(),
    )?))
}

fn resolve_cwd(cwd: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    cwd.unwrap_or(std::env::current_dir()?)
        .canonicalize()
        .context("working directory is unavailable")
}

async fn run_headless(
    runner: Arc<dyn TurnRunner>,
    cwd: PathBuf,
    profile: axiomcli::app::PermissionProfile,
    configured_model: String,
    prompt: String,
    store: SessionStore,
) -> anyhow::Result<()> {
    let runtime = Runtime::new(APP_EVENT_QUEUE_CAPACITY);
    let session_id = runtime.session_id();
    let turn_id = runtime.turn_id();
    let model = store
        .last_used_model()?
        .unwrap_or_else(|| configured_model.clone());
    let thinking = store
        .last_used_thinking()?
        .unwrap_or(axiomcli::app::ThinkingLevel::Medium);
    let created = runtime
        .dispatch(AppCommand::CreateSession {
            session_id: session_id.clone(),
            cwd: cwd.clone(),
            origin: Origin::Headless,
            profile,
        })
        .await?;
    store.append_all(&created)?;
    if model != configured_model {
        runner.set_model(&session_id, model.clone()).await?;
    }
    if thinking != axiomcli::app::ThinkingLevel::Medium {
        runner.set_thinking_level(&session_id, thinking).await?;
    }
    let selected_model = runtime
        .dispatch(AppCommand::ChangeModel {
            session_id: session_id.clone(),
            model: model.clone(),
        })
        .await?;
    store.append_all(&selected_model)?;
    let selected_thinking = runtime
        .dispatch(AppCommand::ChangeThinkingLevel {
            session_id: session_id.clone(),
            level: thinking,
        })
        .await?;
    store.append_all(&selected_thinking)?;
    let submitted = runtime
        .dispatch(AppCommand::SubmitPrompt {
            session_id: session_id.clone(),
            turn_id: turn_id.clone(),
            text: prompt.clone(),
        })
        .await?;
    store.append_all(&submitted)?;
    store.set_last_used_model(&model)?;
    store.set_last_used_thinking(thinking)?;

    let title_runner = runner.clone();
    let title_store = store.clone();
    let title_session = session_id.clone();
    let title_prompt = prompt.clone();
    let title_task = tokio::spawn(async move {
        let title = title_runner
            .generate_title(&title_session, &title_prompt, CancellationToken::new())
            .await?;
        title_store.set_title_if_absent(&title_session, &title)?;
        Ok::<(), AxiomError>(())
    });

    let (events_tx, mut events_rx) = mpsc::channel(APP_EVENT_QUEUE_CAPACITY);
    let context = TurnContext {
        session_id: session_id.clone(),
        turn_id: turn_id.clone(),
        cwd,
        permission_profile: profile,
        approval: None,
        questions: None,
    };
    let cancellation = CancellationToken::new();
    let run = runner.run(context, prompt, events_tx, cancellation.clone());
    tokio::pin!(run);
    let result = loop {
        tokio::select! {
            result = &mut run => {
                if result.is_err() {
                    cancellation.cancel();
                }
                break result;
            }
            event = events_rx.recv() => {
                if let Some(event) = event {
                    record_headless_event(&runtime, &store, &session_id, event).await?;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                cancellation.cancel();
            },
        }
    };
    while let Ok(event) = events_rx.try_recv() {
        record_headless_event(&runtime, &store, &session_id, event).await?;
    }

    let terminal = match &result {
        Ok(()) => AppCommand::FinishTurn {
            session_id: session_id.clone(),
            turn_id,
        },
        Err(AxiomError::Cancelled) => AppCommand::CancelTurn {
            session_id: session_id.clone(),
            turn_id,
        },
        Err(error) => AppCommand::FailTurn {
            session_id: session_id.clone(),
            turn_id,
            message: error.to_string(),
        },
    };
    store.append_all(&runtime.dispatch(terminal).await?)?;
    match tokio::time::timeout(std::time::Duration::from_secs(2), title_task).await {
        Ok(Ok(Ok(()))) => {}
        Ok(Ok(Err(error))) => tracing::warn!(%error, "background session title generation failed"),
        Ok(Err(error)) => tracing::warn!(%error, "background session title task failed"),
        Err(_) => tracing::warn!("background session title generation did not finish before exit"),
    }
    result.map_err(anyhow::Error::from)
}

async fn record_headless_event(
    runtime: &Runtime,
    store: &SessionStore,
    session_id: &SessionId,
    event: AppEvent,
) -> anyhow::Result<()> {
    print_headless_event(event.clone());
    let envelope = runtime.emit(session_id.clone(), event).await?;
    store.append(&envelope)?;
    Ok(())
}

fn print_headless_event(event: AppEvent) {
    match event {
        AppEvent::TextDelta { text, .. } => print!("{text}"),
        AppEvent::ReasoningDelta { text, .. } => eprint!("{text}"),
        AppEvent::ToolStarted { name, .. } => eprintln!("\n[tool] {name}"),
        AppEvent::ToolOutput { content, .. } => eprintln!("{content}"),
        AppEvent::ErrorRaised { message, .. } => eprintln!("error: {message}"),
        _ => {}
    }
}
