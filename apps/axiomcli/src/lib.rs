#![recursion_limit = "256"]

//! `AxiomCLI`'s early, intentionally change-friendly implementation.

pub mod account_store;
pub mod acp;
pub mod agent;
pub mod app;
pub mod audit;
pub mod auth;
pub mod billing;
pub mod config;
pub mod error;
pub mod mcp;
pub mod paths;
pub mod planning;
pub mod policy;
mod process_env;
pub mod provider;
pub mod proxy;
pub mod session;
pub mod session_title;
pub mod slash;
pub mod steering;
pub mod tools;
pub mod tui;
pub mod updates;
pub mod web;
pub mod workspace;

mod tool_display;

pub use error::{AxiomError, Result};
