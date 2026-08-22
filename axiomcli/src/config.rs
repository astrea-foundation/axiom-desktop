use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use clap::ValueEnum;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{Result, app::PermissionProfile, error::AxiomError, policy::PolicyRule};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum PermissionProfileArg {
    Observe,
    #[default]
    Confirm,
    Workspace,
    Autonomous,
}

impl From<PermissionProfileArg> for PermissionProfile {
    fn from(value: PermissionProfileArg) -> Self {
        match value {
            PermissionProfileArg::Observe => Self::Observe,
            PermissionProfileArg::Confirm => Self::Confirm,
            PermissionProfileArg::Workspace => Self::Workspace,
            PermissionProfileArg::Autonomous => Self::Autonomous,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub model: String,
    pub base_url: String,
    pub permission_profile: PermissionProfile,
    pub searxng_url: String,
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
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Names explicitly trusted by the user as read-only. Server annotations
    /// are display hints and never grant this policy classification.
    #[serde(default)]
    pub read_only_tools: Vec<String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ConfigLayer {
    model: Option<String>,
    base_url: Option<String>,
    permission_profile: Option<PermissionProfile>,
    searxng_url: Option<String>,
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
        if let Some(value) = self.searxng_url {
            config.searxng_url = value;
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

    fn apply_project(self, config: &mut Config) -> Result<()> {
        if self.base_url.is_some() || self.searxng_url.is_some() || self.mcp_servers.is_some() {
            return Err(AxiomError::Config(
                "project config cannot set network endpoints or MCP server commands; use the trusted user config or environment"
                    .into(),
            ));
        }
        if let Some(profile) = self.permission_profile {
            if profile == PermissionProfile::Plan {
                return Err(AxiomError::Config(
                    "project config cannot enable plan artifact writes; use --plan or trusted user configuration"
                        .into(),
                ));
            }
            if profile_rank(profile) > profile_rank(config.permission_profile) {
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

const fn profile_rank(profile: PermissionProfile) -> u8 {
    match profile {
        PermissionProfile::Plan => 0,
        PermissionProfile::Observe => 1,
        PermissionProfile::Confirm => 2,
        PermissionProfile::Workspace => 3,
        PermissionProfile::Autonomous => 4,
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: "glm-5-2".into(),
            base_url: "http://127.0.0.1:8484/v1".into(),
            permission_profile: PermissionProfile::Confirm,
            searxng_url: "http://127.0.0.1:8888".into(),
            max_agent_steps: None,
            max_context_bytes: 2 * 1024 * 1024,
            max_context_tokens: 256 * 1024,
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
        let mut config = Self::default();
        if let Some(path) = Self::user_path()
            && path.exists()
        {
            read_layer(&path)?.apply_trusted(&mut config);
        }
        let project_path = PathBuf::from(".axiomcli/config.toml");
        if project_path.exists() {
            read_layer(&project_path)?.apply_project(&mut config)?;
        }
        if let Ok(value) = std::env::var("AXIOM_BASE_URL") {
            config.base_url = value;
        }
        if let Ok(value) = std::env::var("AXIOM_MODEL") {
            config.model = value;
        }
        if let Ok(value) = std::env::var("AXIOM_SEARXNG_URL") {
            config.searxng_url = value;
        }
        if let Ok(value) = std::env::var("AXIOM_PERMISSION_PROFILE") {
            config.permission_profile = PermissionProfile::from_str(&value)?;
        }
        config.validate()?;
        Ok(config)
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
        let searxng_url = reqwest::Url::parse(&self.searxng_url)
            .map_err(|error| AxiomError::Config(format!("invalid searxng_url: {error}")))?;
        if !valid_endpoint(&base_url) || !valid_endpoint(&searxng_url) {
            return Err(AxiomError::Config(
                "provider and SearXNG URLs must use http(s), include a host, and contain no embedded credentials"
                    .into(),
            ));
        }
        let mut names = HashSet::new();
        for rule in self.policy_rules.iter().chain(&self.project_policy_rules) {
            rule.validate()?;
        }
        for server in &self.mcp_servers {
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
    pub fn user_path() -> Option<PathBuf> {
        ProjectDirs::from("ai", "AxiomAI", "axiomcli")
            .map(|dirs| dirs.config_dir().join("config.toml"))
    }

    #[must_use]
    pub fn data_dir() -> Option<PathBuf> {
        ProjectDirs::from("ai", "AxiomAI", "axiomcli")
            .map(|dirs| dirs.data_local_dir().to_path_buf())
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

fn read_layer(path: &std::path::Path) -> Result<ConfigLayer> {
    let content = std::fs::read_to_string(path)?;
    toml::from_str(&content)
        .map_err(|error| AxiomError::Config(format!("{}: {error}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            PermissionProfile::from_str("workspace").expect("profile"),
            PermissionProfile::Workspace
        );
        assert!(PermissionProfile::from_str("YOLO").is_err());
    }

    #[test]
    fn partial_layers_preserve_unspecified_values() {
        let mut config = Config::default();
        let layer: ConfigLayer = toml::from_str("model = 'cherry'").expect("layer");
        layer.apply_trusted(&mut config);
        assert_eq!(config.model, "cherry");
        assert_eq!(config.permission_profile, PermissionProfile::Confirm);
        assert_eq!(config.searxng_url, "http://127.0.0.1:8888");
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
            toml::from_str("permission_profile = 'autonomous'").expect("layer");
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
}
