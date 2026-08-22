use std::{path::PathBuf, str::FromStr as _, sync::Arc};

use anyhow::Context as _;
use axiomcli::{
    AxiomError,
    agent::{AgentEngine, BlockingTurnRunner, EchoTurnRunner, TurnContext, TurnRunner},
    app::{AppEvent, SessionId, TurnId},
    config::Config,
    provider::{AxiomProvider, InferenceProvider},
    session::SessionStore,
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
    },
    /// Run one prompt without a terminal interface.
    Exec {
        prompt: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Serve Agent Client Protocol over stdin/stdout.
    Acp,
    /// Check configuration and local dependencies.
    Doctor,
    /// List models exposed by the configured development endpoint.
    Models,
    /// Inspect and manage durable local session journals.
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
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
    let config = Config::load().context("failed to load AxiomCLI configuration")?;
    init_tracing(matches!(cli.command, Some(Command::Acp)));

    match cli.command.unwrap_or(Command::Tui { cwd: None }) {
        Command::Doctor => {
            doctor(&config).await;
            Ok(())
        }
        Command::Models => {
            let provider =
                AxiomProvider::from_environment(&config.base_url, config.request_timeout())?;
            for model in provider.models(CancellationToken::new()).await? {
                println!("{}", model.id);
            }
            Ok(())
        }
        Command::Sessions { action } => manage_sessions(action),
        Command::Tui { cwd } => {
            let cwd = resolve_cwd(cwd)?;
            axiomcli::tui::run(
                build_runner(&config).await?,
                cwd,
                config.model.clone(),
                config.permission_profile,
                axiomcli::tui::TuiOptions::from_environment(config.animation),
                Some(open_store()?),
            )
            .await
            .map_err(anyhow::Error::from)
        }
        Command::Exec { prompt, cwd } => {
            run_headless(
                build_runner(&config).await?,
                resolve_cwd(cwd)?,
                &config,
                prompt,
            )
            .await
        }
        Command::Acp => axiomcli::acp::serve_stdio(
            build_runner(&config).await?,
            config.permission_profile,
            Some(open_store()?),
        )
        .await
        .map_err(anyhow::Error::from),
    }
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

async fn doctor(config: &Config) {
    println!("AxiomCLI development diagnostics");
    println!("model: {}", config.model);
    println!("base URL: {}", config.base_url);
    println!("security: unattested development");
    println!("permission profile: {}", config.permission_profile);
    println!("SearXNG: {}", config.searxng_url);
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

async fn build_runner(config: &Config) -> anyhow::Result<Arc<dyn TurnRunner>> {
    if cfg!(debug_assertions) {
        match std::env::var("AXIOMCLI_TEST_RUNNER").as_deref() {
            Ok("echo") => return Ok(Arc::new(EchoTurnRunner)),
            Ok("blocking") => return Ok(Arc::new(BlockingTurnRunner)),
            _ => {}
        }
    }
    let provider = AxiomProvider::from_environment(&config.base_url, config.request_timeout())?;
    let mut tools = default_registry(config.max_tool_output_bytes, &config.searxng_url)?;
    for tool in
        axiomcli::mcp::connect_tools(&config.mcp_servers, config.max_tool_output_bytes).await?
    {
        tools.register(tool)?;
    }
    Ok(Arc::new(AgentEngine::new(
        Arc::new(provider),
        Arc::new(tools),
        config.model.clone(),
        config.max_agent_steps,
    )))
}

fn resolve_cwd(cwd: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    cwd.unwrap_or(std::env::current_dir()?)
        .canonicalize()
        .context("working directory is unavailable")
}

async fn run_headless(
    runner: Arc<dyn TurnRunner>,
    cwd: PathBuf,
    config: &Config,
    prompt: String,
) -> anyhow::Result<()> {
    let (events_tx, mut events_rx) = mpsc::unbounded_channel();
    let context = TurnContext {
        session_id: SessionId::new(),
        turn_id: TurnId::new(),
        cwd,
        permission_profile: config.permission_profile,
        approval: None,
    };
    let run = runner.run(context, prompt, events_tx, CancellationToken::new());
    tokio::pin!(run);
    loop {
        tokio::select! {
            result = &mut run => {
                result.map_err(anyhow::Error::from)?;
                while let Ok(event) = events_rx.try_recv() {
                    print_headless_event(event);
                }
                return Ok(());
            }
            event = events_rx.recv() => {
                if let Some(event) = event {
                    print_headless_event(event);
                }
            }
            _ = tokio::signal::ctrl_c() => return Err(anyhow::Error::from(AxiomError::Cancelled)),
        }
    }
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
