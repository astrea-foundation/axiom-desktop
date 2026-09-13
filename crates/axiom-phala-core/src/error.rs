use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("attestation failed: {0}")]
    Attestation(String),
    #[error("relay error: {0}")]
    Relay(String),
    #[error("provider error: {0}")]
    Provider(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
