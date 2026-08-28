#![recursion_limit = "256"]

use std::{
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
};

use anyhow::Context as _;
use axiomcli::{
    AxiomError,
    agent::{
        APP_EVENT_QUEUE_CAPACITY, AgentEngine, AgentLimits, ApprovalTurnRunner, BlockingTurnRunner,
        EchoTurnRunner, MAX_CUSTOM_SYSTEM_PROMPT_BYTES, PlanReviewTurnRunner, QuestionTurnRunner,
        TurnContext, TurnRunner, WorkspaceEditTurnRunner,
    },
    app::{AppCommand, AppEvent, Origin, Runtime, SessionId},
    auth::{AuthManager, ValidationStatus},
    config::Config,
    paths::{AxiomPaths, FrontendKind},
    planning::PlanArtifact,
    policy::{EffectClass, PolicyRule, RuleAction},
    provider::{InferenceProvider, SecureAxiomProvider},
    session::{ExportOptions, SessionStore},
    tools::default_registry,
};
use clap::{Parser, Subcommand};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Parser)]
#[command(name = "axiomcli", version, about = "Early AxiomAI coding-agent TUI")]
struct Cli {
    /// Load agent instructions from a UTF-8 file for this process.
    #[arg(long, global = true, value_name = "PATH")]
    system_prompt_file: Option<PathBuf>,
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
    },
    /// Run one prompt without a terminal interface.
    Exec {
        prompt: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Serve Agent Client Protocol over stdin/stdout.
    Acp {
        /// Select the policy, storage, prompt, and workspace contract for the ACP client.
        #[arg(long, value_enum, default_value_t = FrontendKind::Cli)]
        frontend: FrontendKind,
    },
    /// Check configuration and local dependencies.
    Doctor,
    /// List models exposed by the configured secure Axiom service.
    Models,
    /// Delete one pre-release frontend state database after explicit confirmation.
    ResetLocalState {
        #[arg(long, value_enum)]
        frontend: FrontendKind,
        /// Confirm permanent deletion of this frontend's local threads and collections.
        #[arg(long)]
        confirm: bool,
    },
    /// Inspect and manage durable local threads.
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
        max_items: usize,
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let frontend = match &cli.command {
        Some(Command::Acp { frontend }) => *frontend,
        _ => FrontendKind::Cli,
    };
    anyhow::ensure!(
        frontend != FrontendKind::DesktopChat || cli.system_prompt_file.is_none(),
        "--system-prompt-file cannot replace the Axiom Desktop product prompt"
    );
    let system_prompt = cli
        .system_prompt_file
        .as_deref()
        .map(load_system_prompt_file)
        .transpose()?;
    let paths = AxiomPaths::discover().context("failed to resolve Axiom platform paths")?;
    paths
        .prepare()
        .context("failed to prepare Axiom platform paths")?;
    let config =
        Config::load_for(frontend, &paths).context("failed to load AxiomCLI configuration")?;
    let auth = AuthManager::new_with_paths(&config.base_url, config.request_timeout(), &paths)
        .context("failed to initialize Axiom authentication")?;
    let tui_mode = matches!(cli.command, None | Some(Command::Tui { .. }));
    init_tracing(tui_mode);

    match cli.command.unwrap_or(Command::Tui {
        cwd: None,
        resume: None,
    }) {
        Command::Doctor => {
            doctor(&config, &auth, &paths).await;
            Ok(())
        }
        Command::Models => {
            let provider = build_provider(&config, &auth)?;
            for model in provider.models(CancellationToken::new()).await? {
                println!("{}", model.id);
            }
            Ok(())
        }
        Command::ResetLocalState { frontend, confirm } => {
            anyhow::ensure!(confirm, "refusing to delete local state without --confirm");
            let removed = paths
                .reset_frontend_state(frontend)
                .context("failed to reset frontend local state")?;
            if removed.is_empty() {
                println!("{frontend} local state is already empty");
            } else {
                println!(
                    "removed {} {frontend} local state file(s); shared identity, credentials, trust policy, configuration, and chat workspace were preserved",
                    removed.len()
                );
            }
            Ok(())
        }
        Command::Sessions { action } => manage_sessions(action, &paths),
        Command::Plans { cwd, action } => manage_plans(&resolve_cwd(cwd)?, action),
        Command::Permissions { action } => manage_permissions(action),
        Command::Tui { cwd, resume } => {
            let store = open_store(&paths, FrontendKind::Cli)?;
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
                (saved, Some(profile))
            } else {
                (resolve_cwd(cwd)?, None)
            };
            let workspace_config = config
                .for_workspace(&cwd)
                .context("failed to load workspace configuration")?;
            let profile = saved_profile.unwrap_or(workspace_config.permission_profile);
            axiomcli::tui::run(
                build_runner(
                    &workspace_config,
                    &auth,
                    system_prompt.as_deref(),
                    FrontendKind::Cli,
                )
                .await?,
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
        Command::Exec { prompt, cwd } => {
            let cwd = resolve_cwd(cwd)?;
            let workspace_config = config
                .for_workspace(&cwd)
                .context("failed to load workspace configuration")?;
            let profile = workspace_config.permission_profile;
            run_headless(
                build_runner(
                    &workspace_config,
                    &auth,
                    system_prompt.as_deref(),
                    FrontendKind::Cli,
                )
                .await?,
                cwd,
                profile,
                workspace_config.model.clone(),
                prompt,
                open_store(&paths, FrontendKind::Cli)?,
            )
            .await
        }
        Command::Acp { frontend } => axiomcli::acp::serve_stdio(
            build_runner(&config, &auth, system_prompt.as_deref(), frontend).await?,
            config.clone(),
            Some(open_store(&paths, frontend)?),
            auth,
            frontend,
            if frontend == FrontendKind::DesktopChat {
                Some(paths.desktop_chat_cwd()?)
            } else {
                None
            },
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

fn init_tracing(tui_mode: bool) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    if tui_mode {
        // Direct stderr writes corrupt Ratatui's alternate-screen renderer.
        // Actionable failures are delivered through typed UI messages; traces
        // remain available in ACP and non-interactive CLI modes.
        builder.with_writer(std::io::sink).init();
    } else {
        builder.with_writer(std::io::stderr).init();
    }
}

async fn doctor(config: &Config, auth: &AuthManager, paths: &AxiomPaths) {
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
    println!("shared config: {}", paths.shared_config_path().display());
    println!("Axiom data root: {}", paths.data_root().display());
    println!(
        "CLI data: {}",
        paths.frontend_data_dir(FrontendKind::Cli).display()
    );
    println!(
        "desktop data: {}",
        paths.frontend_data_dir(FrontendKind::DesktopChat).display()
    );
    println!(
        "local state database: {}",
        state_path(paths, FrontendKind::Cli).display()
    );
    match paths.desktop_chat_cwd() {
        Ok(path) => println!("desktop chat workspace: {}", path.display()),
        Err(error) => println!("desktop chat workspace: unavailable ({error})"),
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

fn state_path(paths: &AxiomPaths, frontend: FrontendKind) -> PathBuf {
    paths.state_database(frontend)
}

fn open_store(paths: &AxiomPaths, frontend: FrontendKind) -> anyhow::Result<SessionStore> {
    SessionStore::open(state_path(paths, frontend)).map_err(Into::into)
}

fn manage_sessions(action: SessionAction, paths: &AxiomPaths) -> anyhow::Result<()> {
    let store = open_store(paths, FrontendKind::Cli)?;
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
            max_items,
        } => {
            let id = SessionId::from_str(&id).context("invalid session ID")?;
            println!(
                "{}",
                store.export_with_options(
                    &id,
                    ExportOptions {
                        include_prompts: !omit_prompts,
                        include_tool_output: !omit_tool_output,
                        max_items,
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
    }
    Ok(())
}

async fn build_runner(
    config: &Config,
    auth: &AuthManager,
    system_prompt: Option<&str>,
    frontend: FrontendKind,
) -> anyhow::Result<Arc<dyn TurnRunner>> {
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
    if frontend == FrontendKind::Cli {
        for tool in
            axiomcli::mcp::connect_tools(&config.mcp_servers, config.max_tool_output_bytes).await?
        {
            tools.register(tool)?;
        }
    }
    let policy_rules = config.policy_rules.clone();
    let mut engine = AgentEngine::with_limits(
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
    );
    if frontend == FrontendKind::DesktopChat {
        engine =
            engine.with_system_prompt(include_str!("../prompts/desktop-chat.md").to_owned())?;
    } else if let Some(system_prompt) = system_prompt {
        engine = engine.with_system_prompt(system_prompt.to_owned())?;
    }
    Ok(Arc::new(engine.with_policy_rules(policy_rules)))
}

fn load_system_prompt_file(path: &Path) -> anyhow::Result<String> {
    let file = File::open(path)
        .with_context(|| format!("failed to open system prompt file `{}`", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_CUSTOM_SYSTEM_PROMPT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("failed to read system prompt file `{}`", path.display()))?;
    anyhow::ensure!(
        bytes.len() <= MAX_CUSTOM_SYSTEM_PROMPT_BYTES,
        "system prompt file `{}` exceeds the 256 KiB limit",
        path.display()
    );
    let prompt = String::from_utf8(bytes)
        .with_context(|| format!("system prompt file `{}` is not valid UTF-8", path.display()))?;
    let prompt = prompt
        .strip_prefix('\u{feff}')
        .unwrap_or(&prompt)
        .to_owned();
    anyhow::ensure!(
        !prompt.trim().is_empty(),
        "system prompt file `{}` is empty",
        path.display()
    );
    anyhow::ensure!(
        !prompt.contains('\0'),
        "system prompt file `{}` contains a NUL byte",
        path.display()
    );
    Ok(prompt)
}

fn build_provider(
    config: &Config,
    auth: &AuthManager,
) -> anyhow::Result<Arc<dyn InferenceProvider>> {
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
