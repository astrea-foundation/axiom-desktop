use std::{collections::HashSet, path::PathBuf, str::FromStr, time::Duration};

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
    pub max_agent_steps: usize,
    pub max_context_bytes: usize,
    pub max_context_tokens: usize,
    pub max_tool_output_bytes: usize,
    pub max_turn_secs: u64,
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
            config.max_agent_steps = value;
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
            config.max_turn_secs = value;
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
            config.max_agent_steps = value.min(config.max_agent_steps);
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
            config.max_turn_secs = value.min(config.max_turn_secs);
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
            max_agent_steps: 24,
            max_context_bytes: 2 * 1024 * 1024,
            max_context_tokens: 256 * 1024,
            max_tool_output_bytes: 128 * 1024,
            max_turn_secs: 15 * 60,
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
        if self.max_agent_steps == 0 || self.max_agent_steps > 256 {
            return Err(AxiomError::Config(
                "max_agent_steps must be between 1 and 256".into(),
            ));
        }
        if self.max_tool_output_bytes < 1024 {
            return Err(AxiomError::Config(
                "max_tool_output_bytes must be at least 1024".into(),
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
        if self.max_turn_secs == 0 || self.max_turn_secs > 24 * 60 * 60 {
            return Err(AxiomError::Config(
                "max_turn_secs must be between 1 and 86400".into(),
            ));
        }
        let base_url = reqwest::Url::parse(&self.base_url)
            .map_err(|error| AxiomError::Config(format!("invalid base_url: {error}")))?;
        let searxng_url = reqwest::Url::parse(&self.searxng_url)
            .map_err(|error| AxiomError::Config(format!("invalid searxng_url: {error}")))?;
        if !matches!(base_url.scheme(), "http" | "https")
            || !matches!(searxng_url.scheme(), "http" | "https")
        {
            return Err(AxiomError::Config(
                "provider and SearXNG URLs must use http or https".into(),
            ));
        }
        let mut names = HashSet::new();
        for rule in self.policy_rules.iter().chain(&self.project_policy_rules) {
            rule.validate()?;
        }
        for server in &self.mcp_servers {
            if server.name.trim().is_empty() || server.command.trim().is_empty() {
                return Err(AxiomError::Config(
                    "MCP server name and command cannot be empty".into(),
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
}
