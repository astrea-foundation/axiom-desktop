#![recursion_limit = "256"]

//! `AxiomCLI`'s early, intentionally change-friendly implementation.

pub mod acp;
pub mod agent;
pub mod app;
pub mod audit;
pub mod auth;
pub mod config;
pub mod error;
pub mod mcp;
pub mod planning;
pub mod policy;
pub mod provider;
pub mod session;
pub mod slash;
pub mod tools;
pub mod tui;
pub mod web;
pub mod workspace;

pub use error::{AxiomError, Result};
