//! Immediate local labels and account-bound background titles for new threads.

use crate::{
    AxiomError, Result,
    agent::TurnRunner,
    app::{AppEvent, SessionId},
    session::SessionStore,
};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// An account-bound, single-use reservation for a new thread's automatic title.
pub struct TitleGeneration {
    store: SessionStore,
    session_id: SessionId,
    generation: String,
    prompt: String,
    model: String,
}

impl TitleGeneration {
    pub fn prepare(
        store: &SessionStore,
        session_id: &SessionId,
        prompt: &str,
        model: &str,
    ) -> Result<Option<Self>> {
        let store = store.bind_active_account()?;
        let fallback = session_title(prompt);
        if prompt.trim().is_empty() {
            store.set_title_if_absent(session_id, &fallback)?;
            return Ok(None);
        }
        let Some(generation) = store.begin_title_generation(session_id, &fallback)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            store,
            session_id: session_id.clone(),
            generation,
            prompt: prompt.into(),
            model: model.into(),
        }))
    }

    pub async fn run(
        self,
        runner: &dyn TurnRunner,
        cancellation: CancellationToken,
    ) -> Result<Option<String>> {
        let cancellation = cancellation.child_token();
        let _cancel_on_drop = cancellation.clone().drop_guard();
        if cancellation.is_cancelled()
            || !self
                .store
                .title_generation_pending(&self.session_id, &self.generation)?
        {
            let _ = self
                .store
                .finish_title_generation(&self.session_id, &self.generation, None);
            return Ok(None);
        }
        let (tx, mut rx) = mpsc::channel(64);
        let request = runner.generate_title(&self.model, &self.prompt, tx, cancellation.clone());
        tokio::pin!(request);
        let deadline = tokio::time::sleep(Duration::from_secs(60));
        tokio::pin!(deadline);
        let mut ticks = tokio::time::interval(Duration::from_millis(250));
        let mut stopping = false;
        let mut events_open = true;
        let result = loop {
            tokio::select! {
                result = &mut request => break result,
                event = rx.recv(), if events_open => {
                    if let Some(event) = event { self.record_usage(event)?; } else { events_open = false; }
                }
                () = cancellation.cancelled(), if !stopping => {
                    stopping = true;
                    deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(2));
                }
                () = &mut deadline => {
                    if stopping { break Err(AxiomError::Cancelled); }
                    cancellation.cancel();
                    stopping = true;
                    deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(2));
                }
                _ = ticks.tick(), if !stopping => {
                    if !self.store.title_generation_pending(&self.session_id, &self.generation).unwrap_or(false) {
                        cancellation.cancel();
                    }
                }
            }
        };
        while let Ok(event) = rx.try_recv() {
            self.record_usage(event)?;
        }
        let title = match result {
            Ok(title) if !cancellation.is_cancelled() => title,
            result => {
                let _ =
                    self.store
                        .finish_title_generation(&self.session_id, &self.generation, None);
                return result.and(Ok(None));
            }
        };
        let changed =
            self.store
                .finish_title_generation(&self.session_id, &self.generation, Some(&title))?;
        Ok(changed.then_some(title))
    }

    fn record_usage(&self, event: AppEvent) -> Result<()> {
        if let AppEvent::RequestUsageUpdated { mut usage } = event {
            usage.purpose = axiom_inference::InvocationPurpose::Title;
            self.store.record_request_usage(&self.session_id, &usage)?;
        }
        Ok(())
    }
}

/// Derive a display title without a provider request or background task.
#[must_use]
pub fn session_title(prompt: &str) -> String {
    let line = prompt
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let safe_line = line
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>();
    let line = safe_line.trim().trim_matches(|character: char| {
        matches!(character, '"' | '\'' | '`' | '#' | '*' | '_' | '[' | ']')
    });
    let mut title = line
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    let mut boundary = title.len().min(80);
    while !title.is_char_boundary(boundary) {
        boundary -= 1;
    }
    title.truncate(boundary);
    let title = title
        .trim()
        .trim_end_matches(['.', ',', ':', ';', '!', '?', '-', '—'])
        .trim();
    if title.is_empty() {
        "Conversation".into()
    } else {
        title.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::session_title;

    #[test]
    fn labels_use_the_first_nonempty_line_and_at_most_eight_words() {
        assert_eq!(
            session_title("\n  \"Repair the workspace.\"\nExtra context"),
            "Repair the workspace"
        );
        assert_eq!(
            session_title("Please repair the workspace without changing its theme today"),
            "Please repair the workspace without changing its theme"
        );
    }

    #[test]
    fn labels_are_bounded_valid_utf8_without_control_characters() {
        let title = session_title(&format!("\u{1b}{}", "界".repeat(40)));
        assert_eq!(title, "界".repeat(26));
        assert!(title.len() <= 80);
        assert_eq!(session_title("\t\n***?!***"), "Conversation");
        assert_eq!(session_title(""), "Conversation");
    }
}
