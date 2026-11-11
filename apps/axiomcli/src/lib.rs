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
pub mod desktop_mcp;
pub mod error;
pub mod factory_reset;
pub mod image_tool;
pub(crate) mod local_storage;
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
pub mod storage_backup;
pub mod tools;
pub mod tui;
pub mod updates;
pub mod web;
pub mod workspace;

mod tool_display;

pub use error::{AxiomError, Result};
