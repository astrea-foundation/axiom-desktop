use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{AxiomError, Result, app::PermissionProfile};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Effect {
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
    },
    Network {
        url: String,
    },
    Mcp {
        server: String,
        tool: String,
        side_effecting: bool,
    },
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub explanation: String,
    pub effect: Effect,
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
        }
    }
}

#[async_trait]
pub trait ApprovalHandler: Send + Sync {
    async fn request(
        &self,
        request: ApprovalRequest,
        cancellation: CancellationToken,
    ) -> Result<bool>;
}

#[derive(Debug, Default)]
pub struct DenyApproval;

#[async_trait]
impl ApprovalHandler for DenyApproval {
    async fn request(
        &self,
        _request: ApprovalRequest,
        _cancellation: CancellationToken,
    ) -> Result<bool> {
        Ok(false)
    }
}

#[derive(Clone, Debug)]
pub struct PolicyEngine {
    roots: Vec<PathBuf>,
}

impl PolicyEngine {
    pub fn for_workspace(root: &Path) -> Result<Self> {
        Ok(Self {
            roots: vec![root.canonicalize()?],
        })
    }

    #[must_use]
    pub fn evaluate(&self, profile: PermissionProfile, effect: Effect) -> PolicyDecision {
        if let Some(reason) = self.hard_denial(&effect) {
            return PolicyDecision {
                kind: DecisionKind::Deny,
                explanation: reason,
                normalized_effect: effect,
            };
        }

        let (kind, explanation) = match (profile, &effect) {
            (_, Effect::FileRead { .. }) => (DecisionKind::Allow, "workspace read"),
            (PermissionProfile::Observe, _) => (DecisionKind::Deny, "observe mode is read-only"),
            (PermissionProfile::Confirm, _) => {
                (DecisionKind::Ask, "confirm mode requires approval")
            }
            (PermissionProfile::Workspace, Effect::FileWrite { .. }) => {
                (DecisionKind::Allow, "workspace mode permits in-root edits")
            }
            (PermissionProfile::Workspace, Effect::FileDelete { .. }) => {
                (DecisionKind::Ask, "file deletion requires approval")
            }
            (
                PermissionProfile::Workspace,
                Effect::Mcp {
                    side_effecting: false,
                    ..
                },
            ) => (DecisionKind::Allow, "explicitly trusted read-only MCP tool"),
            (
                PermissionProfile::Workspace,
                Effect::Process {
                    program,
                    args,
                    shell: false,
                },
            ) if is_workspace_program_allowed(program, args) => {
                (DecisionKind::Allow, "allowlisted development command")
            }
            (PermissionProfile::Workspace, _) => {
                (DecisionKind::Ask, "operation exceeds workspace defaults")
            }
            (PermissionProfile::Autonomous, Effect::Process { shell: true, .. }) => (
                DecisionKind::Ask,
                "shell interpretation always requires approval",
            ),
            (PermissionProfile::Autonomous, Effect::FileDelete { .. }) => {
                (DecisionKind::Ask, "deletion remains confirmation-gated")
            }
            (PermissionProfile::Autonomous, _) => {
                (DecisionKind::Allow, "allowed by autonomous profile")
            }
        };
        PolicyDecision {
            kind,
            explanation: explanation.into(),
            normalized_effect: effect,
        }
    }

    fn hard_denial(&self, effect: &Effect) -> Option<String> {
        match effect {
            Effect::FileRead { path }
            | Effect::FileWrite { path }
            | Effect::FileDelete { path } => {
                if self.is_in_roots(path) {
                    None
                } else {
                    Some(format!(
                        "{} is outside authorized workspace roots",
                        path.display()
                    ))
                }
            }
            Effect::Process { program, .. }
                if matches!(
                    program.rsplit('/').next().unwrap_or(program),
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

    fn is_in_roots(&self, path: &Path) -> bool {
        let normalized = normalize_for_policy(path);
        self.roots.iter().any(|root| normalized.starts_with(root))
    }
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

fn is_workspace_program_allowed(program: &str, args: &[String]) -> bool {
    let program = program.rsplit('/').next().unwrap_or(program);
    match program {
        "cargo" => matches!(
            args.first().map(String::as_str),
            Some("check" | "test" | "fmt" | "clippy")
        ),
        "npm" | "pnpm" | "yarn" => matches!(args.first().map(String::as_str), Some("test" | "run")),
        "git" => matches!(
            args.first().map(String::as_str),
            Some("status" | "diff" | "log" | "show")
        ),
        _ => false,
    }
}

#[must_use]
pub fn describe_effect(effect: &Effect) -> String {
    match effect {
        Effect::FileRead { path } => format!("read {}", path.display()),
        Effect::FileWrite { path } => format!("write {}", path.display()),
        Effect::FileDelete { path } => format!("delete {}", path.display()),
        Effect::Process {
            program,
            args,
            shell,
        } => format!(
            "run {}{}{}",
            if *shell { "shell " } else { "" },
            program,
            if args.is_empty() {
                String::new()
            } else {
                format!(" {}", args.join(" "))
            }
        ),
        Effect::Network { url } => format!("connect to {url}"),
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
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn deny_precedes_all_profiles() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
        for profile in [
            PermissionProfile::Observe,
            PermissionProfile::Confirm,
            PermissionProfile::Workspace,
            PermissionProfile::Autonomous,
        ] {
            let decision = policy.evaluate(
                profile,
                Effect::FileWrite {
                    path: root.path().parent().expect("parent").join("escape.txt"),
                },
            );
            assert_eq!(decision.kind, DecisionKind::Deny);
        }
    }

    #[test]
    fn profile_matrix_is_narrow() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
        let path = root.path().join("file.rs");
        assert_eq!(
            policy
                .evaluate(
                    PermissionProfile::Observe,
                    Effect::FileRead { path: path.clone() }
                )
                .kind,
            DecisionKind::Allow
        );
        assert_eq!(
            policy
                .evaluate(
                    PermissionProfile::Observe,
                    Effect::FileWrite { path: path.clone() }
                )
                .kind,
            DecisionKind::Deny
        );
        assert_eq!(
            policy
                .evaluate(
                    PermissionProfile::Confirm,
                    Effect::FileWrite { path: path.clone() }
                )
                .kind,
            DecisionKind::Ask
        );
        assert_eq!(
            policy
                .evaluate(PermissionProfile::Workspace, Effect::FileWrite { path })
                .kind,
            DecisionKind::Allow
        );
    }

    #[test]
    fn privileged_program_is_hard_denied() {
        let root = tempdir().expect("root");
        let policy = PolicyEngine::for_workspace(root.path()).expect("policy");
        let decision = policy.evaluate(
            PermissionProfile::Autonomous,
            Effect::Process {
                program: "sudo".into(),
                args: vec!["true".into()],
                shell: false,
            },
        );
        assert_eq!(decision.kind, DecisionKind::Deny);
    }
}
