use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    app::PermissionProfile,
    error::AxiomError,
    paths::{AxiomPaths, FrontendKind},
    policy::PolicyRule,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfileArg {
    None,
    Web,
    Observe,
    #[default]
    Confirm,
    FullAccess,
}

impl From<PermissionProfileArg> for PermissionProfile {
    fn from(value: PermissionProfileArg) -> Self {
        match value {
            PermissionProfileArg::None => Self::None,
            PermissionProfileArg::Web => Self::Web,
            PermissionProfileArg::Observe => Self::Observe,
            PermissionProfileArg::Confirm => Self::Confirm,
            PermissionProfileArg::FullAccess => Self::FullAccess,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: String,
    pub base_url: String,
    pub permission_profile: PermissionProfile,
    pub web_search_provider: String,
    pub max_agent_steps: Option<usize>,
    pub max_context_bytes: usize,
    pub max_context_tokens: usize,
    pub max_tool_output_bytes: usize,
    pub max_turn_secs: Option<u64>,
    pub request_timeout_secs: u64,
    pub animation: bool,
    pub policy_rules: Vec<PolicyRule>,
    /// Restrictions loaded from the current repository. Kept separate so a
    /// project layer can never replace trusted-user restrictions.
    #[serde(skip)]
    pub project_policy_rules: Vec<PolicyRule>,
    pub mcp_servers: Vec<McpServerConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Names explicitly trusted by the user as read-only. Server annotations
    /// are display hints and never grant this policy classification.
    #[serde(default)]
    pub read_only_tools: Vec<String>,
    /// Trusted per-server tool deadline; discovery keeps a separate short timeout.
    #[serde(default = "default_mcp_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
}

#[must_use]
pub const fn default_mcp_tool_timeout_secs() -> u64 {
    300
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ConfigLayer {
    model: Option<String>,
    base_url: Option<String>,
    permission_profile: Option<PermissionProfile>,
    web_search_provider: Option<String>,
    max_agent_steps: Option<usize>,
    max_context_bytes: Option<usize>,
    max_context_tokens: Option<usize>,
    max_tool_output_bytes: Option<usize>,
    max_turn_secs: Option<u64>,
    request_timeout_secs: Option<u64>,
    animation: Option<bool>,
    policy_rules: Option<Vec<PolicyRule>>,
    mcp_servers: Option<Vec<McpServerConfig>>,
}

impl ConfigLayer {
    fn apply_trusted(self, config: &mut Config) {
        if let Some(value) = self.model {
            config.model = value;
        }
        if let Some(value) = self.base_url {
            config.base_url = value;
        }
        if let Some(value) = self.permission_profile {
            config.permission_profile = value;
        }
        if let Some(value) = self.web_search_provider {
            config.web_search_provider = value;
        }
        if let Some(value) = self.max_agent_steps {
            config.max_agent_steps = Some(value);
        }
        if let Some(value) = self.max_context_bytes {
            config.max_context_bytes = value;
        }
        if let Some(value) = self.max_context_tokens {
            config.max_context_tokens = value;
        }
        if let Some(value) = self.max_tool_output_bytes {
            config.max_tool_output_bytes = value;
        }
        if let Some(value) = self.max_turn_secs {
            config.max_turn_secs = Some(value);
        }
        if let Some(value) = self.request_timeout_secs {
            config.request_timeout_secs = value;
        }
        if let Some(value) = self.animation {
            config.animation = value;
        }
        if let Some(value) = self.policy_rules {
            config.policy_rules = value;
        }
        if let Some(value) = self.mcp_servers {
            config.mcp_servers = value;
        }
    }

    fn apply_desktop_shared(mut self, config: &mut Config) {
        // The shared file is also the CLI's trusted configuration. Desktop
        // consumes its common fields, but these two CLI-only authority
        // surfaces cannot alter the fixed Web/no-MCP desktop product profile.
        self.permission_profile = None;
        self.mcp_servers = None;
        self.apply_trusted(config);
    }

    fn apply_project(self, config: &mut Config) -> Result<()> {
        if self.base_url.is_some()
            || self.web_search_provider.is_some()
            || self.mcp_servers.is_some()
        {
            return Err(AxiomError::Config(
                "project config cannot set network endpoints or MCP server commands; use the trusted user config or environment"
                    .into(),
            ));
        }
        if let Some(profile) = self.permission_profile {
            if !profile_is_narrower_or_equal(profile, config.permission_profile) {
                return Err(AxiomError::Config(
                    "project config cannot broaden the trusted permission profile".into(),
                ));
            }
            config.permission_profile = profile;
        }
        if let Some(value) = self.model {
            config.model = value;
        }
        if let Some(value) = self.max_agent_steps {
            config.max_agent_steps = Some(
                config
                    .max_agent_steps
                    .map_or(value, |trusted| value.min(trusted)),
            );
        }
        if let Some(value) = self.max_context_bytes {
            config.max_context_bytes = value.min(config.max_context_bytes);
        }
        if let Some(value) = self.max_context_tokens {
            config.max_context_tokens = value.min(config.max_context_tokens);
        }
        if let Some(value) = self.max_tool_output_bytes {
            config.max_tool_output_bytes = value.min(config.max_tool_output_bytes);
        }
        if let Some(value) = self.max_turn_secs {
            config.max_turn_secs = Some(
                config
                    .max_turn_secs
                    .map_or(value, |trusted| value.min(trusted)),
            );
        }
        if let Some(value) = self.request_timeout_secs {
            config.request_timeout_secs = value.min(config.request_timeout_secs);
        }
        if let Some(value) = self.animation {
            config.animation = value;
        }
        if let Some(value) = self.policy_rules {
            config.project_policy_rules = value;
        }
        Ok(())
    }
}

const fn profile_is_narrower_or_equal(
    candidate: PermissionProfile,
    trusted: PermissionProfile,
) -> bool {
    match trusted {
        PermissionProfile::None => matches!(candidate, PermissionProfile::None),
        PermissionProfile::Web => {
            matches!(candidate, PermissionProfile::None | PermissionProfile::Web)
        }
        PermissionProfile::Observe => {
            matches!(
                candidate,
                PermissionProfile::None | PermissionProfile::Observe
            )
        }
        PermissionProfile::Confirm => !matches!(candidate, PermissionProfile::FullAccess),
        PermissionProfile::FullAccess => true,
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "auto".into(),
            base_url: "https://api.axiom.stream".into(),
            permission_profile: PermissionProfile::Confirm,
            web_search_provider: "axiom".into(),
            max_agent_steps: None,
            // Provider model metadata owns the normal context/compaction
            // limit. These deliberately larger values are only local memory
            // safety ceilings and may still be lowered by trusted config.
            max_context_bytes: 16 * 1024 * 1024,
            max_context_tokens: 4 * 1024 * 1024,
            max_tool_output_bytes: 128 * 1024,
            max_turn_secs: None,
            request_timeout_secs: 120,
            animation: true,
            policy_rules: Vec::new(),
            project_policy_rules: Vec::new(),
            mcp_servers: Vec::new(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let paths = AxiomPaths::discover()?;
        Self::load_for(FrontendKind::Cli, &paths)
    }

    pub fn load_for(frontend: FrontendKind, paths: &AxiomPaths) -> Result<Self> {
        let mut config = Self::default();
        let shared_path = paths.shared_config_path();
        if shared_path.exists() {
            let layer = read_layer(&shared_path)?;
            if frontend == FrontendKind::DesktopChat {
                layer.apply_desktop_shared(&mut config);
            } else {
                layer.apply_trusted(&mut config);
            }
        }
        let frontend_path = paths.frontend_config_path(frontend);
        if frontend_path.exists() {
            let layer = read_layer(&frontend_path)?;
            if frontend == FrontendKind::DesktopChat
                && layer
                    .permission_profile
                    .is_some_and(|profile| profile != PermissionProfile::Web)
            {
                return Err(AxiomError::Config(format!(
                    "{}: desktop chat permission_profile is fixed to web",
                    frontend_path.display()
                )));
            }
            layer.apply_trusted(&mut config);
        }
        if frontend == FrontendKind::DesktopChat
            && let Ok(profile) = std::env::var("AXIOM_PERMISSION_PROFILE")
            && profile != "web"
        {
            return Err(AxiomError::Config(
                "AXIOM_PERMISSION_PROFILE cannot override desktop chat; use web".into(),
            ));
        }
        apply_environment(&mut config)?;
        if frontend == FrontendKind::DesktopChat {
            // This is a product invariant, not a mutable user preference.
            config.permission_profile = PermissionProfile::Web;
            config.mcp_servers.clear();
        }
        config.validate()?;
        Ok(config)
    }

    /// Apply the untrusted restrictions belonging to `workspace`, rather than
    /// whichever directory happened to launch the process.
    pub fn for_workspace(&self, workspace: &Path) -> Result<Self> {
        let mut config = self.clone();
        config.project_policy_rules.clear();
        let project_path = workspace.join(".axiomcli/config.toml");
        if project_path.exists() {
            read_layer(&project_path)?.apply_project(&mut config)?;
        }
        // Environment values are the final trusted override, matching the
        // documented layer order even though the workspace is selected after
        // process-wide configuration is initially loaded.
        apply_environment(&mut config)?;
        config.validate()?;
        Ok(config)
    }

    /// Read only the current workspace's restrictive policy rules. The agent
    /// uses this when one ACP process owns sessions for several workspaces.
    pub fn project_policy_rules_for(workspace: &Path) -> Result<Vec<PolicyRule>> {
        let project_path = workspace.join(".axiomcli/config.toml");
        if !project_path.exists() {
            return Ok(Vec::new());
        }
        let layer = read_layer(&project_path)?;
        // Applying against a maximally permissive trusted profile validates
        // that the project layer contains no authority-granting settings.
        let mut config = Config {
            permission_profile: PermissionProfile::FullAccess,
            ..Config::default()
        };
        layer.apply_project(&mut config)?;
        Ok(config.project_policy_rules)
    }

    pub fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            return Err(AxiomError::Config("model cannot be empty".into()));
        }
        if let Some(max_agent_steps) = self.max_agent_steps
            && (max_agent_steps == 0 || max_agent_steps > 256)
        {
            return Err(AxiomError::Config(
                "max_agent_steps must be between 1 and 256 when enabled".into(),
            ));
        }
        if !(1024..=64 * 1024 * 1024).contains(&self.max_tool_output_bytes) {
            return Err(AxiomError::Config(
                "max_tool_output_bytes must be between 1024 and 67108864".into(),
            ));
        }
        if self.max_context_bytes < 4096 || self.max_context_bytes > 64 * 1024 * 1024 {
            return Err(AxiomError::Config(
                "max_context_bytes must be between 4096 and 67108864".into(),
            ));
        }
        if self.max_context_tokens < 1024 || self.max_context_tokens > 4 * 1024 * 1024 {
            return Err(AxiomError::Config(
                "max_context_tokens must be between 1024 and 4194304".into(),
            ));
        }
        if let Some(max_turn_secs) = self.max_turn_secs
            && (max_turn_secs == 0 || max_turn_secs > 24 * 60 * 60)
        {
            return Err(AxiomError::Config(
                "max_turn_secs must be between 1 and 86400 when enabled".into(),
            ));
        }
        if !(1..=3600).contains(&self.request_timeout_secs) {
            return Err(AxiomError::Config(
                "request_timeout_secs must be between 1 and 3600".into(),
            ));
        }
        let base_url = reqwest::Url::parse(&self.base_url)
            .map_err(|error| AxiomError::Config(format!("invalid base_url: {error}")))?;
        if self.web_search_provider != "axiom" {
            return Err(AxiomError::Config(
                "web_search_provider must be axiom; other search providers are not supported"
                    .into(),
            ));
        }
        if !valid_endpoint(&base_url) {
            return Err(AxiomError::Config(
                "base_url must use http(s), include a host, and contain no embedded credentials"
                    .into(),
            ));
        }
        let mut names = HashSet::new();
        for rule in self.policy_rules.iter().chain(&self.project_policy_rules) {
            rule.validate()?;
        }
        for server in &self.mcp_servers {
            if !(1..=86_400).contains(&server.tool_timeout_secs) {
                return Err(AxiomError::Config(
                    "MCP tool_timeout_secs must be between 1 and 86400".into(),
                ));
            }
            if server.name.trim().is_empty()
                || server.command.trim().is_empty()
                || server.name.contains('\0')
                || server.command.contains('\0')
                || server.args.iter().any(|argument| argument.contains('\0'))
                || server
                    .read_only_tools
                    .iter()
                    .any(|tool| tool.trim().is_empty() || tool.contains('\0'))
            {
                return Err(AxiomError::Config(
                    "MCP server fields must be non-empty where required and contain no NUL bytes"
                        .into(),
                ));
            }
            if !names.insert(&server.name) {
                return Err(AxiomError::Config(format!(
                    "duplicate MCP server name `{}`",
                    server.name
                )));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.request_timeout_secs)
    }

    #[must_use]
    pub fn user_dir() -> Option<PathBuf> {
        AxiomPaths::discover()
            .ok()
            .map(|paths| paths.frontend_config_dir(FrontendKind::Cli))
    }

    #[must_use]
    pub fn user_path() -> Option<PathBuf> {
        Self::user_dir().map(|directory| directory.join("config.toml"))
    }

    /// Persist one restrictive project policy rule through an explicit user
    /// action. Project configuration can only add `ask`/`deny` rules, never an
    /// authority-granting rule.
    pub fn add_project_rule(workspace: &Path, rule: PolicyRule) -> Result<PathBuf> {
        rule.validate()?;
        let root = workspace.canonicalize()?;
        let directory = root.join(".axiomcli");
        std::fs::create_dir_all(&directory)?;
        let directory = directory.canonicalize()?;
        if !directory.starts_with(&root) {
            return Err(AxiomError::OutsideWorkspace(directory));
        }
        let path = directory.join("config.toml");
        if path.exists() {
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(AxiomError::Config(
                    "project config must be a regular non-symlink file".into(),
                ));
            }
        }
        let mut document = if path.exists() {
            toml::from_str::<toml::Value>(&std::fs::read_to_string(&path)?)
                .map_err(|error| AxiomError::Config(format!("{}: {error}", path.display())))?
        } else {
            toml::Value::Table(toml::map::Map::new())
        };
        let table = document.as_table_mut().ok_or_else(|| {
            AxiomError::Config("project configuration root must be a TOML table".into())
        })?;
        let encoded = toml::Value::try_from(rule)
            .map_err(|error| AxiomError::Config(format!("cannot encode policy rule: {error}")))?;
        match table.get_mut("policy_rules") {
            Some(toml::Value::Array(rules)) => rules.push(encoded),
            Some(_) => {
                return Err(AxiomError::Config(
                    "project policy_rules must be an array of tables".into(),
                ));
            }
            None => {
                table.insert("policy_rules".into(), toml::Value::Array(vec![encoded]));
            }
        }
        let temporary = directory.join(format!(".config.{}.tmp", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let encoded = toml::to_string_pretty(&document).map_err(|error| {
            AxiomError::Config(format!("cannot encode project config: {error}"))
        })?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(path)
    }
}

fn valid_endpoint(url: &reqwest::Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

fn apply_environment(config: &mut Config) -> Result<()> {
    if let Ok(value) = std::env::var("AXIOM_BASE_URL") {
        config.base_url = value;
    }
    if let Ok(value) = std::env::var("AXIOM_MODEL") {
        config.model = value;
    }
    if let Ok(value) = std::env::var("AXIOM_WEB_SEARCH_PROVIDER") {
        config.web_search_provider = value;
    }
    if let Ok(value) = std::env::var("AXIOM_PERMISSION_PROFILE") {
        config.permission_profile = PermissionProfile::from_str(&value)?;
    }
    Ok(())
}

fn read_layer(path: &std::path::Path) -> Result<ConfigLayer> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AxiomError::Config(format!(
            "{} must be a regular non-symlink file",
            path.display()
        )));
    }
    let content = std::fs::read_to_string(path)?;
    warn_about_unknown_config_fields(path, &content)?;
    toml::from_str(&content)
        .map_err(|error| AxiomError::Config(format!("{}: {error}", path.display())))
}

fn warn_about_unknown_config_fields(path: &Path, content: &str) -> Result<()> {
    const TOP_LEVEL_FIELDS: &[&str] = &[
        "model",
        "base_url",
        "permission_profile",
        "web_search_provider",
        "max_agent_steps",
        "max_context_bytes",
        "max_context_tokens",
        "max_tool_output_bytes",
        "max_turn_secs",
        "request_timeout_secs",
        "animation",
        "policy_rules",
        "mcp_servers",
    ];
    const MCP_FIELDS: &[&str] = &[
        "name",
        "command",
        "args",
        "read_only_tools",
        "tool_timeout_secs",
    ];

    let document: toml::Value = toml::from_str(content)
        .map_err(|error| AxiomError::Config(format!("{}: {error}", path.display())))?;
    let Some(table) = document.as_table() else {
        return Err(AxiomError::Config(format!(
            "{}: configuration root must be a TOML table",
            path.display()
        )));
    };
    for field in table.keys() {
        if !TOP_LEVEL_FIELDS.contains(&field.as_str()) {
            tracing::warn!(path = %path.display(), field, "unknown config field was ignored");
        }
    }
    if let Some(toml::Value::Array(servers)) = table.get("mcp_servers") {
        for (index, server) in servers.iter().enumerate() {
            if let Some(server) = server.as_table() {
                for field in server.keys() {
                    if !MCP_FIELDS.contains(&field.as_str()) {
                        tracing::warn!(
                            path = %path.display(),
                            index,
                            field,
                            "unknown MCP config field was ignored"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_config_lives_in_the_shared_axiom_cli_directory() {
        let directory = Config::user_dir().expect("platform config directory");
        let path = Config::user_path().expect("user config path");

        assert!(directory.ends_with(Path::new("axiom/cli")));
        assert_eq!(path, directory.join("config.toml"));
    }

    #[test]
    fn desktop_configuration_is_layered_but_web_policy_is_immutable() {
        let root = tempfile::tempdir().expect("temp dir");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare paths");
        std::fs::write(
            paths.frontend_config_path(FrontendKind::DesktopChat),
            "model = \"desktop-model\"\npermission_profile = \"web\"\n",
        )
        .expect("desktop config");

        for (profile, expected) in [
            ("confirm", PermissionProfile::Confirm),
            ("full_access", PermissionProfile::FullAccess),
        ] {
            std::fs::write(
                paths.shared_config_path(),
                format!(
                    "model = \"shared-model\"\npermission_profile = \"{profile}\"\nrequest_timeout_secs = 321\n[[mcp_servers]]\nname = \"shared-tools\"\ncommand = \"shared-command\"\n"
                ),
            )
            .expect("shared config");

            let desktop =
                Config::load_for(FrontendKind::DesktopChat, &paths).expect("desktop config");
            assert_eq!(desktop.model, "desktop-model");
            assert_eq!(desktop.request_timeout_secs, 321);
            assert_eq!(desktop.permission_profile, PermissionProfile::Web);
            assert!(desktop.mcp_servers.is_empty());

            let cli = Config::load_for(FrontendKind::Cli, &paths).expect("CLI config");
            assert_eq!(cli.model, "shared-model");
            assert_eq!(cli.request_timeout_secs, 321);
            assert_eq!(cli.permission_profile, expected);
            assert_eq!(cli.mcp_servers.len(), 1);
            assert_eq!(cli.mcp_servers[0].name, "shared-tools");
        }

        std::fs::write(
            paths.frontend_config_path(FrontendKind::DesktopChat),
            "permission_profile = \"confirm\"\n",
        )
        .expect("invalid desktop config");
        assert!(Config::load_for(FrontendKind::DesktopChat, &paths).is_err());

        std::fs::write(
            paths.frontend_config_path(FrontendKind::DesktopChat),
            "permission_profile = \"web\"\n",
        )
        .expect("restored desktop config");
        std::fs::write(paths.shared_config_path(), "unknown_shared_field = true\n")
            .expect("additive shared config");
        let desktop = Config::load_for(FrontendKind::DesktopChat, &paths)
            .expect("unknown additive keys do not block desktop");
        assert_eq!(desktop.permission_profile, PermissionProfile::Web);
    }

    #[test]
    fn invalid_urls_fail_validation() {
        let config = Config {
            base_url: "not a url".into(),
            ..Config::default()
        };
        assert!(config.validate().is_err());
        for url in ["ftp://example.com", "https://user:secret@example.com"] {
            let config = Config {
                base_url: url.into(),
                ..Config::default()
            };
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn unsupported_search_providers_fail_instead_of_changing_the_destination() {
        assert!(Config::default().validate().is_ok());
        for provider in ["retired-provider", "", "AXIOM"] {
            let mut config = Config::default();
            let layer: ConfigLayer = toml::from_str(&format!("web_search_provider = {provider:?}"))
                .expect("provider config");
            layer.apply_trusted(&mut config);
            let error = config.validate().expect_err("unsupported provider");
            assert!(
                error
                    .to_string()
                    .contains("web_search_provider must be axiom")
            );
        }
    }

    #[test]
    fn release_resource_limits_are_bounded() {
        for config in [
            Config {
                request_timeout_secs: 0,
                ..Config::default()
            },
            Config {
                request_timeout_secs: 3601,
                ..Config::default()
            },
            Config {
                max_tool_output_bytes: 64 * 1024 * 1024 + 1,
                ..Config::default()
            },
        ] {
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn profile_parser_is_strict() {
        assert_eq!(
            PermissionProfile::from_str("full_access").expect("profile"),
            PermissionProfile::FullAccess
        );
        assert!(PermissionProfile::from_str("workspace").is_err());
        assert!(PermissionProfile::from_str("autonomous").is_err());
        assert!(PermissionProfile::from_str("plan").is_err());
        assert!(PermissionProfile::from_str("YOLO").is_err());
    }

    #[test]
    fn partial_layers_preserve_unspecified_values() {
        let mut config = Config::default();
        let layer: ConfigLayer = toml::from_str("model = 'cherry'").expect("layer");
        layer.apply_trusted(&mut config);
        assert_eq!(config.model, "cherry");
        assert_eq!(config.permission_profile, PermissionProfile::Confirm);
        assert_eq!(config.web_search_provider, "axiom");
    }

    #[test]
    fn additive_config_and_mcp_fields_are_ignored_without_losing_known_values() {
        let root = tempfile::tempdir().expect("config root");
        let path = root.path().join("config.toml");
        std::fs::write(
            &path,
            "model='cherry'\nfuture_ui_setting=true\n[[mcp_servers]]\nname='future'\ncommand='helper'\nfuture_transport='v2'\n",
        )
        .expect("config fixture");
        let layer = read_layer(&path).expect("additive config");
        let mut config = Config::default();
        layer.apply_trusted(&mut config);
        assert_eq!(config.model, "cherry");
        assert_eq!(config.mcp_servers[0].name, "future");
    }

    #[test]
    fn agent_step_and_wall_time_limits_default_off_and_can_be_enabled() {
        let mut config = Config::default();
        assert_eq!(config.max_agent_steps, None);
        assert_eq!(config.max_turn_secs, None);

        let trusted: ConfigLayer =
            toml::from_str("max_agent_steps = 64\nmax_turn_secs = 3600\n").expect("layer");
        trusted.apply_trusted(&mut config);
        assert_eq!(config.max_agent_steps, Some(64));
        assert_eq!(config.max_turn_secs, Some(3600));
        config.validate().expect("enabled optional limits");
    }

    #[test]
    fn project_limits_can_only_add_or_narrow_optional_limits() {
        let mut unlimited = Config::default();
        let project: ConfigLayer =
            toml::from_str("max_agent_steps = 40\nmax_turn_secs = 1200\n").expect("layer");
        project
            .apply_project(&mut unlimited)
            .expect("project adds restriction");
        assert_eq!(unlimited.max_agent_steps, Some(40));
        assert_eq!(unlimited.max_turn_secs, Some(1200));

        let mut limited = Config {
            max_agent_steps: Some(20),
            max_turn_secs: Some(600),
            ..Config::default()
        };
        let project: ConfigLayer =
            toml::from_str("max_agent_steps = 40\nmax_turn_secs = 1200\n").expect("layer");
        project
            .apply_project(&mut limited)
            .expect("project preserves tighter trusted limits");
        assert_eq!(limited.max_agent_steps, Some(20));
        assert_eq!(limited.max_turn_secs, Some(600));
    }

    #[test]
    fn project_layer_cannot_broaden_authority_or_launch_mcp() {
        let mut config = Config::default();
        let broad: ConfigLayer =
            toml::from_str("permission_profile = 'full_access'").expect("layer");
        assert!(broad.apply_project(&mut config).is_err());

        let mcp: ConfigLayer =
            toml::from_str("[[mcp_servers]]\nname='bad'\ncommand='arbitrary'\n").expect("layer");
        assert!(mcp.apply_project(&mut config).is_err());

        let narrow: ConfigLayer = toml::from_str("permission_profile = 'observe'").expect("layer");
        narrow.apply_project(&mut config).expect("narrowing");
        assert_eq!(config.permission_profile, PermissionProfile::Observe);
    }

    #[test]
    fn project_policy_only_adds_restrictions_and_cannot_replace_global_rules() {
        let mut config = Config::default();
        let trusted: ConfigLayer = toml::from_str(
            "[[policy_rules]]\naction='deny'\neffect='network'\nreason='trusted deny'\n",
        )
        .expect("trusted rule");
        trusted.apply_trusted(&mut config);
        let project: ConfigLayer = toml::from_str(
            "[[policy_rules]]\naction='ask'\neffect='file_write'\nresource_prefix='src'\n",
        )
        .expect("project rule");
        project.apply_project(&mut config).expect("narrowing rule");
        assert_eq!(config.policy_rules.len(), 1);
        assert_eq!(config.project_policy_rules.len(), 1);
        assert!(
            toml::from_str::<ConfigLayer>("[[policy_rules]]\naction='allow'\neffect='process'\n")
                .is_err()
        );
    }

    #[test]
    fn explicit_project_rule_flow_appends_only_restrictive_rules() {
        let root = tempfile::tempdir().expect("workspace");
        let path = Config::add_project_rule(
            root.path(),
            PolicyRule {
                action: crate::policy::RuleAction::Ask,
                effect: Some(crate::policy::EffectClass::Network),
                resource_prefix: Some("https://example.com/".into()),
                reason: Some("fixture review".into()),
            },
        )
        .expect("first rule");
        Config::add_project_rule(
            root.path(),
            PolicyRule {
                action: crate::policy::RuleAction::Deny,
                effect: Some(crate::policy::EffectClass::CredentialAccess),
                resource_prefix: None,
                reason: Some("never read credentials".into()),
            },
        )
        .expect("second rule");
        let layer: ConfigLayer = read_layer(&path).expect("project layer");
        let rules = layer.policy_rules.expect("rules");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].action, crate::policy::RuleAction::Ask);
        assert_eq!(rules[1].action, crate::policy::RuleAction::Deny);
    }

    #[test]
    fn workspace_configuration_is_loaded_from_the_selected_workspace() {
        let selected = tempfile::tempdir().expect("selected workspace");
        let launch = tempfile::tempdir().expect("launch workspace");
        std::fs::create_dir_all(selected.path().join(".axiomcli")).expect("config directory");
        std::fs::write(
            selected.path().join(".axiomcli/config.toml"),
            "permission_profile='observe'\n[[policy_rules]]\naction='ask'\neffect='network'\n",
        )
        .expect("project config");

        let trusted = Config::default();
        let selected_config = trusted
            .for_workspace(selected.path())
            .expect("selected config");
        let launch_config = trusted.for_workspace(launch.path()).expect("launch config");

        assert_eq!(
            selected_config.permission_profile,
            PermissionProfile::Observe
        );
        assert_eq!(selected_config.project_policy_rules.len(), 1);
        assert_eq!(launch_config.permission_profile, PermissionProfile::Confirm);
        assert!(launch_config.project_policy_rules.is_empty());
    }
}
