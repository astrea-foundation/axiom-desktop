//! Input routing shared by keyboard and bracketed-paste interactions.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    policy::{ApprovalChoice, ApprovalRequest},
    slash::SlashCommand,
};

use super::{
    state::{Focus, InteractionView, Overlay, SavePickerFocus, TuiState},
    text::sanitize_terminal_text,
};

pub(super) fn is_text_input(modifiers: KeyModifiers) -> bool {
    modifiers.is_empty() || modifiers == KeyModifiers::SHIFT
}

// Approvals can arrive while the user is typing steering. Ordinary letters must
// never become authorization decisions when that asynchronous card takes focus.
pub(super) fn approval_choice(key: KeyEvent, request: &ApprovalRequest) -> Option<ApprovalChoice> {
    if key.modifiers != KeyModifiers::CONTROL {
        return None;
    }
    match key.code {
        KeyCode::Char('y' | 'Y') => Some(ApprovalChoice::AllowOnce),
        KeyCode::Char('n' | 'N') => Some(ApprovalChoice::Deny),
        KeyCode::Char('g' | 'G') if request.allow_session_grants => {
            Some(ApprovalChoice::AllowExactSession)
        }
        KeyCode::Char('p' | 'P')
            if request.allow_session_grants && request.suggested_prefix_scope.is_some() =>
        {
            Some(ApprovalChoice::AllowPrefixSession)
        }
        _ => None,
    }
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
            Some(Overlay::Billing(view)) => view.paste(&text),
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
