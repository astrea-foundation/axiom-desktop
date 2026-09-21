//! Input routing shared by keyboard and bracketed-paste interactions.

use crossterm::event::KeyModifiers;

use crate::slash::SlashCommand;

use super::{
    state::{Focus, InteractionView, Overlay, SavePickerFocus, TuiState},
    text::sanitize_terminal_text,
};

pub(super) fn is_text_input(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

pub(super) fn command_allowed_while_running(command: &SlashCommand) -> bool {
    matches!(
        command,
        SlashCommand::Help | SlashCommand::Usage | SlashCommand::Security | SlashCommand::Theme(_)
    )
}

impl TuiState {
    pub(super) fn paste_text(&mut self, text: &str) {
        self.exit_armed = false;
        let text = sanitize_terminal_text(text);
        let single_line = || text.replace(['\n', '\r'], " ");
        match &self.interaction {
            Some(InteractionView::Questions { .. }) => {
                self.answer_input.push_str(&single_line());
                return;
            }
            Some(InteractionView::Approval(_)) => return,
            None => {}
        }
        match &mut self.overlay {
            Some(
                Overlay::ModelPicker { query, selected }
                | Overlay::ResumePicker {
                    query, selected, ..
                }
                | Overlay::DeletePicker {
                    query,
                    selected,
                    confirming: false,
                    ..
                },
            ) => {
                query.push_str(&single_line());
                *selected = 0;
            }
            Some(Overlay::SaveAttestation(picker)) if picker.focus == SavePickerFocus::Filename => {
                self.push_attestation_filename(&text.replace(['\n', '\r'], ""));
            }
            None if self.search_active => {
                self.search_query.push_str(&single_line());
                self.refresh_search();
            }
            None if self.focus == Focus::Composer => {
                self.reset_slash_completion();
                self.input.insert_str(&text);
            }
            // Non-editable overlays must not change the hidden composer.
            Some(_) | None => {}
        }
    }
}
