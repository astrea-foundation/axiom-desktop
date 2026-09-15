//! Axiom's in-tree, fail-closed verifier and receipt-gated cipher for the two
//! pinned Phala ACI evidence profiles.

pub mod error;
pub mod provider;
pub mod providers;
pub mod relay;

pub use error::{CoreError, Result};
