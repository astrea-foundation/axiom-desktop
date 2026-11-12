//! Usage presentation from native reports, independent of provider/model names.

use std::fmt::Write as _;

use axiom_inference::{InvocationPurpose, InvocationState};

use crate::app::TurnId;

use super::{state::TuiState, text::sanitize_terminal_text};

impl TuiState {
    pub(super) fn context_summary(&self) -> String {
        let Some(usage) = &self.context_usage else {
            return "Context unknown".into();
        };
        let Some(capacity) = usage.context_window_tokens.filter(|capacity| *capacity > 0) else {
            return "Context size unknown".into();
        };
        let used = u128::from(usage.input_tokens) + u128::from(usage.output_tokens);
        format!("Context {}%", used * 100 / u128::from(capacity))
    }

    pub(super) fn usage_details(&self) -> String {
        let Some(usage) = &self.context_usage else {
            return "No completed request usage has been reported yet.\n\nContext use and compaction threshold are unknown until a report is available.".into();
        };
        let mut text = format!(
            "Last reported conversation request\n\nModel: {}\nReported: {}\nInput: {} tokens\nOutput: {} tokens\nTotal: {} tokens\n",
            sanitize_terminal_text(&usage.model_id),
            sanitize_terminal_text(&usage.reported_at),
            usage.input_tokens,
            usage.output_tokens,
            u128::from(usage.input_tokens) + u128::from(usage.output_tokens),
        );
        if let Some(capacity) = usage.context_window_tokens.filter(|capacity| *capacity > 0) {
            let _ = writeln!(
                text,
                "Window: {capacity} tokens ({})",
                self.context_summary()
            );
            if let Some(threshold) = usage
                .auto_compact_threshold_tokens
                .filter(|threshold| *threshold > 0 && *threshold <= capacity)
            {
                let _ = writeln!(
                    text,
                    "Auto-compacts at: {threshold} tokens ({}%)",
                    u64::from(threshold) * 100 / u64::from(capacity)
                );
            } else {
                text.push_str("Compaction threshold unavailable\n");
            }
        } else {
            text.push_str("Context size and compaction threshold unavailable\n");
        }
        if usage.model_id != self.model {
            text.push_str("\nThis report belongs to the previous model. Changing models does not rescale its counts.\n");
        }
        if self.running {
            text.push_str(
                "\nCurrent work is still running; this is the last completed request report.\n",
            );
        }
        text.push_str("\nReported usage is not a draft estimate or a sum of charges. The compaction threshold does not cap response length.");
        text
    }

    /// Accounting carries the protocol-authenticated finish reason. Settlement
    /// alone must never turn cancelled or unverified output into a completed reply.
    pub(super) fn response_hit_output_limit(&self, turn: &TurnId) -> bool {
        let turn_id = turn.to_string();
        self.request_usage
            .values()
            .filter(|usage| {
                usage.purpose == InvocationPurpose::Conversation
                    && usage.turn_id.as_deref() == Some(turn_id.as_str())
            })
            .max_by_key(|usage| {
                (
                    usage.started_at_ms.parse::<u64>().unwrap_or(0),
                    &usage.request_id,
                )
            })
            .is_some_and(|usage| {
                usage.state == InvocationState::Completed
                    && usage.response_verified
                    && usage.finish_reason.as_deref() == Some("length")
            })
    }
}
