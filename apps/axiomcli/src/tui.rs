//! Terminal frontend. State, views, and asynchronous orchestration have separate owners.

use std::time::Duration;

mod attestation;
mod billing;
mod brand;
mod composer;
mod input;
mod markdown;
mod proof_refresh;
mod runtime;
mod screens;
mod state;
mod task;
mod text;
mod theme;
mod transcript;
mod usage;

pub use runtime::run;
pub use screens::render;
pub use state::{TuiLaunch, TuiOptions, TuiState};
pub use text::sanitize_terminal_text;
pub use theme::{Appearance, ColorMode};

/// Every region of the shell is inset by the same number of columns so text,
/// dividers and the composer all line up on one left edge.
const GUTTER: u16 = 2;
/// Columns reserved for the role marker; entry bodies are indented to match so
/// wrapped continuations stay under the first line instead of falling to zero.
const BODY_INDENT: usize = 2;
/// Smallest terminal the full layout is designed for.
const MIN_WIDTH: u16 = 42;
const MIN_HEIGHT: u16 = 10;
const COMPOSER_MAX_CONTENT_ROWS: u16 = 8;
const ANIMATION_TICK: Duration = Duration::from_millis(160);
fn pick_splash_message() -> &'static str {
    "Think freely."
}

#[cfg(test)]
mod tests;
