use std::path::PathBuf;

/// Stable error categories used across front ends.
#[derive(Debug, thiserror::Error)]
pub enum AxiomError {
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("invalid state transition: {0}")]
    InvalidTransition(String),
    #[error("session not found: {0}")]
    SessionNotFound(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("you declined this action; stopped without running it")]
    ApprovalDeclined,
    #[error("operation requires approval: {0}")]
    ApprovalRequired(String),
    #[error("path is outside the workspace: {0}")]
    OutsideWorkspace(PathBuf),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("{message}")]
    SecureProvider {
        kind: axiom_inference::ProviderFailureKind,
        code: &'static str,
        message: &'static str,
    },
    #[error("tool error: {0}")]
    Tool(String),
    #[error("git error: {0}")]
    Git(#[from] git2::Error),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, AxiomError>;
