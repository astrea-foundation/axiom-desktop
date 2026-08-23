//! Stage 04 proves this module before Stage 05/07 connect it to the provider
//! lifecycle. The allowance disappears once the production driver consumes
//! every entry point.
#![allow(dead_code)]

pub(crate) mod crypto;

pub(crate) const PROVIDER_ID: &str = "near";
pub(crate) const E2EE_PROTOCOL: &str = "near-v2";
pub(crate) const ENCRYPTION_VERSION: u16 = 2;
pub(crate) const ATTESTATION_PROTOCOL: &str = "near-tdx-nvidia-v1";
