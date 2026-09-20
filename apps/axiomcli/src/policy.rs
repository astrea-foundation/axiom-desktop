use std::{
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{AxiomError, Result, app::PermissionProfile};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAccess {
    Interaction,
    Web,
    Read,
    Write,
}

impl ToolAccess {
    #[must_use]
    pub const fn is_exposed_to(self, profile: PermissionProfile) -> bool {
        match profile {
            PermissionProfile::None => false,
            PermissionProfile::Web => matches!(self, Self::Web),
            PermissionProfile::Observe => matches!(self, Self::Read | Self::Interaction),
            PermissionProfile::Confirm | PermissionProfile::FullAccess => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Effect {
    ToolUse {
        name: String,
        access: ToolAccess,
    },
    FileRead {
        path: PathBuf,
    },
    FileWrite {
        path: PathBuf,
    },
    FileDelete {
        path: PathBuf,
    },
    Process {
        program: String,
        args: Vec<String>,
        shell: bool,
        cwd: PathBuf,
        env: Vec<(String, String)>,
    },
    RepositoryMutation {
        operation: String,
        cwd: PathBuf,
    },
    Network {
        url: String,
    },
    Mcp {
        server: String,
        tool: String,
        side_effecting: bool,
    },
    ScopeChange {
        scope: String,
    },
    CredentialAccess {
        service: String,
    },
    PlanWrite {
        path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    ToolUse,
    FileRead,
    FileWrite,
    FileDelete,
    Process,
    RepositoryMutation,
    Network,
    Mcp,
    ScopeChange,
    CredentialAccess,
    PlanWrite,
}

impl EffectClass {
    const fn of(effect: &Effect) -> Self {
        match effect {
            Effect::ToolUse { .. } => Self::ToolUse,
            Effect::FileRead { .. } => Self::FileRead,
            Effect::FileWrite { .. } => Self::FileWrite,
            Effect::FileDelete { .. } => Self::FileDelete,
            Effect::Process { .. } => Self::Process,
            Effect::RepositoryMutation { .. } => Self::RepositoryMutation,
            Effect::Network { .. } => Self::Network,
            Effect::Mcp { .. } => Self::Mcp,
            Effect::ScopeChange { .. } => Self::ScopeChange,
            Effect::CredentialAccess { .. } => Self::CredentialAccess,
            Effect::PlanWrite { .. } => Self::PlanWrite,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Ask,
    Deny,
}

/// A trusted-user or project restriction. Early `AxiomCLI` intentionally accepts
/// only `ask` and `deny` rules: a repository may narrow authority, but it cannot
/// grant itself more authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    pub action: RuleAction,
    #[serde(default)]
    pub effect: Option<EffectClass>,
    #[serde(default)]
    pub resource_prefix: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

impl PolicyRule {
    pub fn validate(&self) -> Result<()> {
        if self
            .resource_prefix
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.contains('\0'))
        {
            return Err(AxiomError::Config(
                "policy rule resource_prefix must be non-empty and contain no NUL".into(),
            ));
        }
        if self
            .reason
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.contains('\0'))
        {
            return Err(AxiomError::Config(
                "policy rule reason must be non-empty and contain no NUL".into(),
            ));
        }
        if self.resource_prefix.is_some() && self.effect.is_none() {
            return Err(AxiomError::Config(
                "policy rules with resource_prefix must select an effect class".into(),
            ));
        }
        Ok(())
    }

    fn matches(&self, effect: &Effect) -> bool {
        if self
            .effect
            .is_some_and(|class| class != EffectClass::of(effect))
        {
            return false;
        }
        self.resource_prefix
            .as_deref()
            .is_none_or(|prefix| effect_matches_prefix(effect, prefix))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Allow,
    Ask,
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub kind: DecisionKind,
    pub explanation: String,
    pub normalized_effect: Effect,
    /// False for an explicit configured `ask`: such a rule must remain an ask
    /// even after another operation was approved for the session.
    pub session_grant_allowed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalChoice {
    Deny,
    AllowOnce,
    AllowExactSession,
    AllowPrefixSession,
}

impl ApprovalChoice {
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        !matches!(self, Self::Deny)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deny => "deny",
            Self::AllowOnce => "allow_once",
            Self::AllowExactSession => "allow_exact_session",
            Self::AllowPrefixSession => "allow_prefix_session",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalResponse {
    pub choice: ApprovalChoice,
}

impl ApprovalResponse {
    #[must_use]
    pub const fn deny() -> Self {
        Self {
            choice: ApprovalChoice::Deny,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub explanation: String,
    pub effect: Effect,
    pub allow_session_grants: bool,
    pub suggested_prefix_scope: Option<String>,
}

impl ApprovalRequest {
    #[must_use]
    pub fn new(decision: &PolicyDecision) -> Self {
        Self {
            request_id: Uuid::new_v4().to_string(),
            explanation: format!(
                "{}: {}",
                decision.explanation,
                describe_effect(&decision.normalized_effect)
            ),
            effect: decision.normalized_effect.clone(),
            allow_session_grants: decision.session_grant_allowed,
            suggested_prefix_scope: decision
                .session_grant_allowed
                .then(|| suggested_prefix(&decision.normalized_effect))
                .flatten()
                .map(|scope| describe_grant(&scope)),
        }
    }

    #[must_use]
    pub fn offered_choices(&self) -> Vec<ApprovalChoice> {
        let mut choices = vec![ApprovalChoice::AllowOnce];
        if self.allow_session_grants {
            choices.push(ApprovalChoice::AllowExactSession);
        }
        if self.suggested_prefix_scope.is_some() {
            choices.push(ApprovalChoice::AllowPrefixSession);
        }
        choices.push(ApprovalChoice::Deny);
        choices
    }
}

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
    ) -> Result<ApprovalResponse>;
}

#[derive(Debug, Default)]
pub struct DenyApproval;

#[async_trait]
impl ApprovalHandler for DenyApproval {
    async fn request(
        &self,
        _request: ApprovalRequest,
        _cancellation: CancellationToken,
    ) -> Result<ApprovalResponse> {
        Ok(ApprovalResponse::deny())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScopedGrant {
    Exact(Effect),
    FilePrefix {
        class: EffectClass,
        path: PathBuf,
    },
    NetworkOrigin(String),
    McpServer {
        server: String,
        side_effecting: bool,
    },
}

impl ScopedGrant {
    fn matches(&self, effect: &Effect) -> bool {
        match (self, effect) {
            (Self::Exact(granted), requested) => granted == requested,
            (Self::FilePrefix { class, path }, requested) => {
                *class == EffectClass::of(requested)
                    && effect_path(requested).is_some_and(|requested| requested.starts_with(path))
            }
            (Self::NetworkOrigin(origin), Effect::Network { url }) => {
                network_origin(url).as_deref() == Some(origin)
            }
            (
                Self::McpServer {
                    server,
                    side_effecting,
                },
                Effect::Mcp {
                    server: requested_server,
                    side_effecting: requested_side_effecting,
                    ..
                },
            ) => server == requested_server && side_effecting == requested_side_effecting,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PolicyEngine {
    roots: Vec<PathBuf>,
    rules: Vec<PolicyRule>,
    grants: Arc<RwLock<Vec<ScopedGrant>>>,
}

impl PolicyEngine {
    pub fn for_workspace(root: &Path) -> Result<Self> {
        Self::for_workspace_with_rules(root, Vec::new())
    }

    pub fn for_workspace_with_rules(root: &Path, rules: Vec<PolicyRule>) -> Result<Self> {
        let root = root.canonicalize()?;
        let rules = rules
            .into_iter()
            .map(|rule| normalize_rule(rule, &root))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            roots: vec![root],
            rules,
            grants: Arc::new(RwLock::new(Vec::new())),
        })
    }

    #[must_use]
    pub fn evaluate(&self, profile: PermissionProfile, effect: Effect) -> PolicyDecision {
        let normalized = match self.normalize(effect.clone()) {
            Ok(effect) => effect,
            Err(error) => return decision(DecisionKind::Deny, error.to_string(), effect, false),
        };
        if let Some(reason) = self.hard_denial(&normalized) {
            return decision(DecisionKind::Deny, reason, normalized, false);
        }

        let matches: Vec<_> = self
            .rules
            .iter()
            .filter(|rule| rule.matches(&normalized))
            .collect();
        if let Some(rule) = matches.iter().find(|rule| rule.action == RuleAction::Deny) {
            return decision(
                DecisionKind::Deny,
                rule.reason
                    .clone()
                    .unwrap_or_else(|| "denied by a configured policy rule".into()),
                normalized,
                false,
            );
        }

        // Restricted tool profiles are hard intersections. A configured `ask`
        // rule must never turn a tool that is absent from the profile into an
        // approval prompt.
        if matches!(
            profile,
            PermissionProfile::None | PermissionProfile::Web | PermissionProfile::Observe
        ) {
            let (kind, explanation, grantable) = profile_default(profile, &normalized);
            if kind == DecisionKind::Deny {
                return decision(kind, explanation.into(), normalized, grantable);
            }
        }

        if let Some(rule) = matches.iter().find(|rule| rule.action == RuleAction::Ask) {
            return decision(
                DecisionKind::Ask,
                rule.reason
                    .clone()
                    .unwrap_or_else(|| "approval required by a configured policy rule".into()),
                normalized,
                false,
            );
        }

        if self
            .grants
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|grant| grant.matches(&normalized))
        {
            return decision(
                DecisionKind::Allow,
                "allowed by a narrow session grant".into(),
                normalized,
                true,
            );
        }

        let (kind, explanation, grantable) = profile_default(profile, &normalized);
        decision(kind, explanation.into(), normalized, grantable)
    }

    pub fn apply_approval(
        &self,
        request: &ApprovalRequest,
        response: ApprovalResponse,
    ) -> Result<()> {
        match response.choice {
            ApprovalChoice::Deny | ApprovalChoice::AllowOnce => Ok(()),
            ApprovalChoice::AllowExactSession | ApprovalChoice::AllowPrefixSession
                if !request.allow_session_grants =>
            {
                Err(AxiomError::PermissionDenied(
                    "this configured approval cannot be converted into a session grant".into(),
                ))
            }
            ApprovalChoice::AllowExactSession => {
                self.ensure_grantable(&request.effect)?;
                self.grants
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(ScopedGrant::Exact(request.effect.clone()));
                Ok(())
            }
            ApprovalChoice::AllowPrefixSession => {
                self.ensure_grantable(&request.effect)?;
                let scope = suggested_prefix(&request.effect).ok_or_else(|| {
                    AxiomError::PermissionDenied(
                        "this operation has no safe predefined prefix scope".into(),
                    )
                })?;
                self.grants
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(scope);
                Ok(())
            }
        }
    }

    fn ensure_grantable(&self, effect: &Effect) -> Result<()> {
        let normalized = self.normalize(effect.clone())?;
        if let Some(reason) = self.hard_denial(&normalized) {
            return Err(AxiomError::PermissionDenied(reason));
        }
        if self.rules.iter().any(|rule| {
            rule.matches(&normalized) && matches!(rule.action, RuleAction::Ask | RuleAction::Deny)
        }) {
            return Err(AxiomError::PermissionDenied(
                "configured ask/deny rules cannot be overridden by a session grant".into(),
            ));
        }
        Ok(())
    }

    fn normalize(&self, effect: Effect) -> Result<Effect> {
        match effect {
            Effect::ToolUse { name, access } => {
                let name = name.trim().to_owned();
                if name.is_empty() || name.contains('\0') {
                    return Err(AxiomError::PermissionDenied(
                        "tool name must be non-empty and NUL-free".into(),
                    ));
                }
                Ok(Effect::ToolUse { name, access })
            }
            Effect::FileRead { path } => Ok(Effect::FileRead {
                path: self.normalize_path(path),
            }),
            Effect::FileWrite { path } => Ok(Effect::FileWrite {
                path: self.normalize_path(path),
            }),
            Effect::FileDelete { path } => Ok(Effect::FileDelete {
                path: self.normalize_path(path),
            }),
            Effect::Process {
                program,
                args,
                shell,
                cwd,
                mut env,
            } => {
                let program = program.trim().to_owned();
                if program.is_empty()
                    || program.contains('\0')
                    || args.iter().any(|argument| argument.contains('\0'))
                    || env.iter().any(|(key, value)| {
                        !is_allowed_child_env_key(key) || key.contains('\0') || value.contains('\0')
                    })
                {
                    return Err(AxiomError::PermissionDenied(
                        "process program/arguments/environment are invalid or not allowlisted"
                            .into(),
                    ));
                }
                env.sort();
                env.dedup_by(|left, right| left.0 == right.0);
                let basename = normalized_executable_name(&program);
                let shell =
                    shell || windows_script_program(&program) || invokes_shell(&basename, &args);
                Ok(Effect::Process {
                    program,
                    args,
                    shell,
                    cwd: self.normalize_path(cwd),
                    env,
                })
            }
            Effect::RepositoryMutation { operation, cwd } => {
                let operation = operation.trim().to_owned();
                if operation.is_empty() || operation.contains('\0') {
                    return Err(AxiomError::PermissionDenied(
                        "repository mutation must have a non-empty/NUL-free operation".into(),
                    ));
                }
                Ok(Effect::RepositoryMutation {
                    operation,
                    cwd: self.normalize_path(cwd),
                })
            }
            Effect::Network { url } => {
                let mut parsed = reqwest::Url::parse(&url).map_err(|error| {
                    AxiomError::PermissionDenied(format!("invalid network destination: {error}"))
                })?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.host_str().is_none()
                {
                    return Err(AxiomError::PermissionDenied(
                        "network destinations require an http(s) URL without embedded credentials"
                            .into(),
                    ));
                }
                parsed.set_fragment(None);
                Ok(Effect::Network {
                    url: parsed.to_string(),
                })
            }
            Effect::Mcp {
                server,
                tool,
                side_effecting,
            } => {
                let server = server.trim().to_owned();
                let tool = tool.trim().to_owned();
                if server.is_empty()
                    || tool.is_empty()
                    || server.contains('\0')
                    || tool.contains('\0')
                {
                    return Err(AxiomError::PermissionDenied(
                        "MCP server and tool names must be non-empty/NUL-free".into(),
                    ));
                }
                Ok(Effect::Mcp {
                    server,
                    tool,
                    side_effecting,
                })
            }
            Effect::ScopeChange { scope } => {
                let scope = scope.trim().to_owned();
                if scope.is_empty() || scope.contains('\0') {
                    return Err(AxiomError::PermissionDenied(
                        "scope changes must have a non-empty/NUL-free identity".into(),
                    ));
                }
                Ok(Effect::ScopeChange { scope })
            }
            Effect::CredentialAccess { service } => {
                let service = service.trim().to_owned();
                if service.is_empty() || service.contains('\0') {
                    return Err(AxiomError::PermissionDenied(
                        "credential service must be non-empty and NUL-free".into(),
                    ));
                }
                Ok(Effect::CredentialAccess { service })
            }
            Effect::PlanWrite { path } => Ok(Effect::PlanWrite {
                path: self.normalize_path(path),
            }),
        }
    }

    fn normalize_path(&self, path: PathBuf) -> PathBuf {
        let absolute = if path.is_absolute() {
            path
        } else {
            self.roots[0].join(path)
        };
        normalize_for_policy(&absolute)
    }

    fn hard_denial(&self, effect: &Effect) -> Option<String> {
        match effect {
            Effect::FileRead { path }
            | Effect::FileWrite { path }
            | Effect::FileDelete { path } => {
                if self.roots.iter().any(|root| path.starts_with(root)) {
                    None
                } else {
                    Some(format!(
                        "{} is outside authorized workspace roots",
                        path.display()
                    ))
                }
            }
            Effect::PlanWrite { path } => {
                if self
                    .roots
                    .iter()
                    .any(|root| path.starts_with(root.join(".axiomcli/plans")))
                {
                    None
                } else {
                    Some(format!(
                        "{} is outside the dedicated plan artifact directory",
                        path.display()
                    ))
                }
            }
            Effect::Process { cwd, .. } if !self.roots.iter().any(|root| cwd.starts_with(root)) => {
                Some(format!(
                    "process cwd {} is outside authorized workspace roots",
                    cwd.display()
                ))
            }
            Effect::RepositoryMutation { cwd, .. }
                if !self.roots.iter().any(|root| cwd.starts_with(root)) =>
            {
                Some(format!(
                    "repository mutation cwd {} is outside authorized workspace roots",
                    cwd.display()
                ))
            }
            Effect::CredentialAccess { .. } => Some(
                "credential/keychain access is unavailable in this early release and hard-denied"
                    .into(),
            ),
            Effect::Process { program, .. }
                if matches!(
                    normalized_executable_name(program).as_str(),
                    "sudo"
                        | "doas"
                        | "su"
                        | "mount"
                        | "umount"
                        | "mkfs"
                        | "shutdown"
                        | "reboot"
                        | "poweroff"
                ) =>
            {
                Some(format!(
                    "privileged/destructive program `{program}` is hard-denied"
                ))
            }
            _ => None,
        }
    }
}

fn normalize_rule(mut rule: PolicyRule, root: &Path) -> Result<PolicyRule> {
    rule.validate()?;
    let Some(prefix) = rule.resource_prefix.take() else {
        return Ok(rule);
    };
    let normalized = match rule.effect.expect("validated effect for prefix") {
        EffectClass::FileRead
        | EffectClass::FileWrite
        | EffectClass::FileDelete
        | EffectClass::PlanWrite => {
            let path = PathBuf::from(prefix);
            let absolute = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            normalize_for_policy(&absolute)
                .to_string_lossy()
                .into_owned()
        }
        EffectClass::Network => {
            let mut url = reqwest::Url::parse(&prefix).map_err(|error| {
                AxiomError::Config(format!("invalid network policy prefix: {error}"))
            })?;
            if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                return Err(AxiomError::Config(
                    "network policy prefixes require an http(s) URL".into(),
                ));
            }
            url.set_fragment(None);
            url.to_string()
        }
        EffectClass::Process
        | EffectClass::ToolUse
        | EffectClass::RepositoryMutation
        | EffectClass::Mcp
        | EffectClass::ScopeChange
        | EffectClass::CredentialAccess => prefix.trim().to_owned(),
    };
    if normalized.is_empty() {
        return Err(AxiomError::Config(
            "normalized policy resource prefix cannot be empty".into(),
        ));
    }
    rule.resource_prefix = Some(normalized);
    Ok(rule)
}

fn decision(
    kind: DecisionKind,
    explanation: String,
    normalized_effect: Effect,
    session_grant_allowed: bool,
) -> PolicyDecision {
    PolicyDecision {
        kind,
        explanation,
        normalized_effect,
        session_grant_allowed,
    }
}

fn profile_default(
    profile: PermissionProfile,
    effect: &Effect,
) -> (DecisionKind, &'static str, bool) {
    match (profile, effect) {
        (PermissionProfile::None, _) => (DecisionKind::Deny, "tools are disabled", false),
        (
            _,
            Effect::ToolUse {
                access: ToolAccess::Interaction,
                ..
            },
        ) => (
            DecisionKind::Allow,
            "the user decides whether to answer",
            false,
        ),
        (
            PermissionProfile::Web,
            Effect::ToolUse {
                access: ToolAccess::Web,
                ..
            }
            | Effect::Network { .. },
        ) => (DecisionKind::Allow, "allowed by web-only profile", false),
        (PermissionProfile::Web, _) => (
            DecisionKind::Deny,
            "web profile permits only web tools",
            false,
        ),
        (
            PermissionProfile::Observe,
            Effect::ToolUse {
                access: ToolAccess::Read,
                ..
            }
            | Effect::FileRead { .. }
            | Effect::Mcp {
                side_effecting: false,
                ..
            },
        ) => (DecisionKind::Allow, "allowed by read-only profile", false),
        (PermissionProfile::Observe, _) => (
            DecisionKind::Deny,
            "observe profile permits only read tools",
            false,
        ),
        (PermissionProfile::Confirm, Effect::ToolUse { .. }) => (
            DecisionKind::Ask,
            "confirm profile requires approval for this tool",
            false,
        ),
        (PermissionProfile::Confirm, _) => (
            DecisionKind::Allow,
            "tool invocation was confirmation-gated",
            false,
        ),
        (PermissionProfile::FullAccess, _) => {
            (DecisionKind::Allow, "allowed by full-access profile", false)
        }
    }
}

fn suggested_prefix(effect: &Effect) -> Option<ScopedGrant> {
    match effect {
        Effect::FileRead { path } | Effect::FileWrite { path } | Effect::FileDelete { path } => {
            Some(ScopedGrant::FilePrefix {
                class: EffectClass::of(effect),
                path: if path.is_dir() {
                    path.clone()
                } else {
                    path.parent()?.to_path_buf()
                },
            })
        }
        Effect::Network { url } => network_origin(url).map(ScopedGrant::NetworkOrigin),
        Effect::Mcp {
            server,
            side_effecting,
            ..
        } => Some(ScopedGrant::McpServer {
            server: server.clone(),
            side_effecting: *side_effecting,
        }),
        Effect::Process { .. }
        | Effect::ToolUse { .. }
        | Effect::RepositoryMutation { .. }
        | Effect::ScopeChange { .. }
        | Effect::CredentialAccess { .. }
        | Effect::PlanWrite { .. } => None,
    }
}

fn describe_grant(grant: &ScopedGrant) -> String {
    match grant {
        ScopedGrant::Exact(effect) => format!("exactly {}", describe_effect(effect)),
        ScopedGrant::FilePrefix { class, path } => {
            format!("{class:?} within {}", path.display())
        }
        ScopedGrant::NetworkOrigin(origin) => format!("network origin {origin}"),
        ScopedGrant::McpServer {
            server,
            side_effecting,
        } => format!(
            "MCP server {server} ({})",
            if *side_effecting {
                "side-effecting tools"
            } else {
                "read-only tools"
            }
        ),
    }
}

fn effect_path(effect: &Effect) -> Option<&Path> {
    match effect {
        Effect::FileRead { path }
        | Effect::FileWrite { path }
        | Effect::FileDelete { path }
        | Effect::PlanWrite { path } => Some(path),
        _ => None,
    }
}

fn effect_matches_prefix(effect: &Effect, prefix: &str) -> bool {
    match effect {
        Effect::ToolUse { name, .. } => name.starts_with(prefix),
        Effect::FileRead { path }
        | Effect::FileWrite { path }
        | Effect::FileDelete { path }
        | Effect::PlanWrite { path } => path.starts_with(Path::new(prefix)),
        Effect::Process { program, .. } => program.starts_with(prefix),
        Effect::RepositoryMutation { operation, .. } => operation.starts_with(prefix),
        Effect::Network { url } => url.starts_with(prefix),
        Effect::Mcp { server, tool, .. } => format!("{server}/{tool}").starts_with(prefix),
        Effect::ScopeChange { scope } => scope.starts_with(prefix),
        Effect::CredentialAccess { service } => service.starts_with(prefix),
    }
}

fn network_origin(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let port = parsed
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    Some(format!("{}://{host}{port}", parsed.scheme()))
}

fn normalize_for_policy(path: &Path) -> PathBuf {
    if path.exists() {
        return path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    }
    let mut ancestor = path;
    let mut missing = Vec::new();
    while !ancestor.exists() {
        if let Some(name) = ancestor.file_name() {
            missing.push(name.to_owned());
        }
        let Some(parent) = ancestor.parent() else {
            return path.to_path_buf();
        };
        ancestor = parent;
    }
    let mut normalized = ancestor
        .canonicalize()
        .unwrap_or_else(|_| ancestor.to_path_buf());
    for component in missing.iter().rev() {
        normalized.push(component);
    }
    normalized
}

#[must_use]
pub fn is_allowed_child_env_key(key: &str) -> bool {
    matches!(
        key,
        "CI" | "RUST_BACKTRACE" | "RUST_LOG" | "NODE_ENV" | "CARGO_TERM_COLOR" | "NO_COLOR"
    )
}

pub(crate) fn normalized_executable_name(program: &str) -> String {
    let trimmed = program.trim().trim_matches(|character: char| {
        matches!(character, '\'' | '"' | '`' | '&' | '(' | ')' | ';' | '|')
    });
    let basename = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
    let lowercase = basename.to_ascii_lowercase();
    for suffix in [".exe", ".com", ".cmd", ".bat"] {
        if let Some(stem) = lowercase.strip_suffix(suffix) {
            return stem.to_owned();
        }
    }
    lowercase
}

fn invokes_shell(program: &str, args: &[String]) -> bool {
    match program {
        "sh" | "bash" | "zsh" | "fish" | "dash" => args.iter().take(2).any(|argument| {
            let argument = argument.to_ascii_lowercase();
            argument.starts_with('-') && argument.contains('c')
        }),
        "cmd" => args
            .iter()
            .take(2)
            .any(|argument| matches!(argument.to_ascii_lowercase().as_str(), "/c" | "/k")),
        "powershell" | "pwsh" => args.iter().any(|argument| {
            matches!(
                argument.to_ascii_lowercase().as_str(),
                "-c" | "-command" | "-ec" | "-encodedcommand" | "-f" | "-file"
            )
        }),
        _ => false,
    }
}

fn windows_script_program(program: &str) -> bool {
    let basename = program
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    [".cmd", ".bat", ".ps1"]
        .iter()
        .any(|extension| basename.ends_with(extension))
}

#[must_use]
pub fn describe_effect(effect: &Effect) -> String {
    match effect {
        Effect::ToolUse { name, .. } => format!("use tool {name}"),
        Effect::FileRead { path } => format!("read {}", path.display()),
        Effect::FileWrite { path } => format!("write {}", path.display()),
        Effect::FileDelete { path } => format!("delete {}", path.display()),
        Effect::Process {
            program,
            args,
            shell,
            cwd,
            env,
        } => format!(
            "run {}{}{} in {}{}",
            if *shell { "shell " } else { "" },
            program,
            if args.is_empty() {
                String::new()
            } else {
                format!(" {}", args.join(" "))
            },
            cwd.display(),
            if env.is_empty() {
                String::new()
            } else {
                format!(
                    " with environment {}",
                    env.iter()
                        .map(|(key, _)| key.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        ),
        Effect::Network { url } => format!("connect to {url}"),
        Effect::RepositoryMutation { operation, cwd } => {
            format!("mutate repository with `{operation}` in {}", cwd.display())
        }
        Effect::Mcp {
            server,
            tool,
            side_effecting,
        } => format!(
            "call MCP {server}/{tool} ({})",
            if *side_effecting {
                "side effects declared"
            } else {
                "read-only declared"
            }
        ),
        Effect::ScopeChange { scope } => format!("expand scope: {scope}"),
        Effect::CredentialAccess { service } => format!("access credentials for {service}"),
        Effect::PlanWrite { path } => format!("update plan artifact {}", path.display()),
    }
}

pub fn require_allowed(decision: &PolicyDecision) -> Result<()> {
    match decision.kind {
        DecisionKind::Allow => Ok(()),
        DecisionKind::Ask => Err(AxiomError::ApprovalRequired(decision.explanation.clone())),
        DecisionKind::Deny => Err(AxiomError::PermissionDenied(decision.explanation.clone())),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use tempfile::tempdir;

    use super::*;

    fn profiles() -> [PermissionProfile; 5] {
        PermissionProfile::ALL
    }

    #[test]
    fn table_covers_profiles_effects_rules_and_scopes() {
        let root = tempdir().expect("root");
        let path = root.path().join("src/lib.rs");
        let effects = [
            Effect::ToolUse {
                name: "read_file".into(),
                access: ToolAccess::Read,
            },
            Effect::FileRead { path: path.clone() },
            Effect::FileWrite { path: path.clone() },
            Effect::FileDelete { path: path.clone() },
            Effect::Process {
                program: "cargo".into(),
                args: vec!["test".into()],
                shell: false,
                cwd: root.path().to_path_buf(),
                env: Vec::new(),
            },
            Effect::Process {
                program: "echo hi".into(),
                args: Vec::new(),
                shell: true,
                cwd: root.path().to_path_buf(),
                env: Vec::new(),
            },
            Effect::RepositoryMutation {
                operation: "git commit".into(),
                cwd: root.path().to_path_buf(),
            },
            Effect::Network {
                url: "https://example.com/search#fragment".into(),
            },
            Effect::Mcp {
                server: "fixture".into(),
                tool: "read".into(),
                side_effecting: false,
            },
            Effect::CredentialAccess {
                service: "fixture-keychain".into(),
            },
            Effect::PlanWrite {
                path: root.path().join(".axiomcli/plans/test.json"),
            },
        ];
        for profile in profiles() {
            let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
            for effect in &effects {
                let decision = policy.evaluate(profile, effect.clone());
                assert_eq!(
                    EffectClass::of(&decision.normalized_effect),
                    EffectClass::of(effect)
                );
                assert!(!decision.explanation.is_empty());
                if decision.kind == DecisionKind::Ask {
                    let request = ApprovalRequest::new(&decision);
                    assert!(
                        request
                            .explanation
                            .contains(&describe_effect(&request.effect))
                    );
                    if request.allow_session_grants {
                        policy
                            .apply_approval(
                                &request,
                                ApprovalResponse {
                                    choice: ApprovalChoice::AllowExactSession,
                                },
                            )
                            .expect("grant");
                        assert_eq!(
                            policy.evaluate(profile, request.effect).kind,
                            DecisionKind::Allow
                        );
                    } else {
                        policy
                            .apply_approval(
                                &request,
                                ApprovalResponse {
                                    choice: ApprovalChoice::AllowOnce,
                                },
                            )
                            .expect("one-shot approval");
                        assert_eq!(
                            policy.evaluate(profile, request.effect).kind,
                            DecisionKind::Ask
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn restricted_profiles_are_hard_tool_intersections() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace_with_rules(
            root.path(),
            vec![PolicyRule {
                action: RuleAction::Ask,
                effect: Some(EffectClass::FileWrite),
                resource_prefix: None,
                reason: None,
            }],
        )
        .expect("policy");
        for (profile, allowed, denied) in [
            (
                PermissionProfile::None,
                None,
                Effect::ToolUse {
                    name: "read_file".into(),
                    access: ToolAccess::Read,
                },
            ),
            (
                PermissionProfile::Web,
                Some(Effect::ToolUse {
                    name: "web_search".into(),
                    access: ToolAccess::Web,
                }),
                Effect::ToolUse {
                    name: "read_file".into(),
                    access: ToolAccess::Read,
                },
            ),
            (
                PermissionProfile::Observe,
                Some(Effect::ToolUse {
                    name: "read_file".into(),
                    access: ToolAccess::Read,
                }),
                Effect::ToolUse {
                    name: "apply_patch".into(),
                    access: ToolAccess::Write,
                },
            ),
        ] {
            if let Some(effect) = allowed {
                assert_eq!(policy.evaluate(profile, effect).kind, DecisionKind::Allow);
            }
            assert_eq!(policy.evaluate(profile, denied).kind, DecisionKind::Deny);
        }
    }

    #[test]
    fn deny_and_explicit_ask_precede_grants_regardless_of_rule_order() {
        let root = tempdir().expect("root");
        let path = root.path().join("file.rs");
        let deny = PolicyRule {
            action: RuleAction::Deny,
            effect: Some(EffectClass::FileWrite),
            resource_prefix: None,
            reason: Some("configured deny".into()),
        };
        let ask = PolicyRule {
            action: RuleAction::Ask,
            effect: Some(EffectClass::FileWrite),
            resource_prefix: None,
            reason: Some("configured ask".into()),
        };
        for rules in [vec![deny.clone(), ask.clone()], vec![ask, deny]] {
            let policy =
                PolicyEngine::for_workspace_with_rules(root.path(), rules).expect("policy");
            assert_eq!(
                policy
                    .evaluate(
                        PermissionProfile::FullAccess,
                        Effect::FileWrite { path: path.clone() }
                    )
                    .kind,
                DecisionKind::Deny
            );
        }
    }

    #[test]
    fn confirm_uses_non_grantable_one_shot_tool_approvals() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
        let decision = policy.evaluate(
            PermissionProfile::Confirm,
            Effect::ToolUse {
                name: "apply_patch".into(),
                access: ToolAccess::Write,
            },
        );
        assert_eq!(decision.kind, DecisionKind::Ask);
        let request = ApprovalRequest::new(&decision);
        assert!(!request.allow_session_grants);
        policy
            .apply_approval(
                &request,
                ApprovalResponse {
                    choice: ApprovalChoice::AllowOnce,
                },
            )
            .expect("one-shot approval");
        assert_eq!(
            policy
                .evaluate(
                    PermissionProfile::Confirm,
                    Effect::ToolUse {
                        name: "apply_patch".into(),
                        access: ToolAccess::Write,
                    }
                )
                .kind,
            DecisionKind::Ask
        );
        assert!(
            policy
                .apply_approval(
                    &request,
                    ApprovalResponse {
                        choice: ApprovalChoice::AllowExactSession,
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn configured_ask_cannot_be_self_broadened_into_a_grant() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace_with_rules(
            root.path(),
            vec![PolicyRule {
                action: RuleAction::Ask,
                effect: Some(EffectClass::Network),
                resource_prefix: None,
                reason: None,
            }],
        )
        .expect("policy");
        let decision = policy.evaluate(
            PermissionProfile::FullAccess,
            Effect::Network {
                url: "https://example.com/a".into(),
            },
        );
        let request = ApprovalRequest::new(&decision);
        assert!(!request.allow_session_grants);
        assert!(
            policy
                .apply_approval(
                    &request,
                    ApprovalResponse {
                        choice: ApprovalChoice::AllowExactSession,
                    },
                )
                .is_err()
        );
    }

    #[test]
    fn hard_denials_and_normalization_fail_closed_for_every_profile() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
        for profile in profiles() {
            for effect in [
                Effect::FileWrite {
                    path: root.path().parent().expect("parent").join("escape.txt"),
                },
                Effect::Process {
                    program: "sudo".into(),
                    args: vec!["true".into()],
                    shell: false,
                    cwd: root.path().to_path_buf(),
                    env: Vec::new(),
                },
                Effect::Network {
                    url: "file:///etc/passwd".into(),
                },
            ] {
                assert_eq!(policy.evaluate(profile, effect).kind, DecisionKind::Deny);
            }
        }
    }

    #[test]
    fn shell_interpretation_is_normalized_but_full_access_needs_no_approval() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
        let disguised_shell = Effect::Process {
            program: "/usr/bin/sh".into(),
            args: vec!["-c".into(), "cargo test".into()],
            shell: false,
            cwd: root.path().to_path_buf(),
            env: Vec::new(),
        };
        let decision = policy.evaluate(PermissionProfile::FullAccess, disguised_shell);
        assert_eq!(decision.kind, DecisionKind::Allow);
        assert!(matches!(
            decision.normalized_effect,
            Effect::Process { shell: true, .. }
        ));

        let structured = Effect::Process {
            program: "cargo".into(),
            args: vec!["test".into()],
            shell: false,
            cwd: root.path().to_path_buf(),
            env: vec![("CI".into(), "true".into())],
        };
        assert_eq!(
            policy.evaluate(PermissionProfile::Confirm, structured).kind,
            DecisionKind::Allow
        );
    }

    #[test]
    fn windows_script_shims_are_shell_classified_without_reinterpreting_arguments() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
        let arguments = vec![
            "run".into(),
            "build & echo not-a-second-command".into(),
            "literal|pipe".into(),
        ];
        let decision = policy.evaluate(
            PermissionProfile::FullAccess,
            Effect::Process {
                program: r"C:\tools\npm.cmd".into(),
                args: arguments.clone(),
                shell: false,
                cwd: root.path().to_path_buf(),
                env: Vec::new(),
            },
        );

        assert_eq!(decision.kind, DecisionKind::Allow);
        assert!(matches!(
            decision.normalized_effect,
            Effect::Process {
                shell: true,
                ref args,
                ..
            } if args == &arguments
        ));
    }

    proptest! {
        #[test]
        fn deny_wins_for_arbitrary_rule_order(order in proptest::collection::vec(any::<bool>(), 1..32)) {
            let root = tempdir().expect("root");
            let mut rules: Vec<_> = order.into_iter().map(|deny| PolicyRule {
                action: if deny { RuleAction::Deny } else { RuleAction::Ask },
                effect: Some(EffectClass::Network),
                resource_prefix: None,
                reason: None,
            }).collect();
            rules.push(PolicyRule {
                action: RuleAction::Deny,
                effect: Some(EffectClass::Network),
                resource_prefix: None,
                reason: None,
            });
            let policy = PolicyEngine::for_workspace_with_rules(root.path(), rules).expect("policy");
            prop_assert_eq!(
                policy.evaluate(PermissionProfile::FullAccess, Effect::Network { url: "https://example.com".into() }).kind,
                DecisionKind::Deny
            );
        }
    }
}
