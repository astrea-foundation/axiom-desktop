use super::sse::RelayEvent;
use std::time::Duration;
use tokio::time::Instant;

/// Keepalive bytes and run metadata cannot keep a stalled model alive forever.
pub(crate) struct StreamDeadline {
    deadline: Instant,
    stage: &'static str,
    finalizing: bool,
}

impl StreamDeadline {
    pub(crate) fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(300),
            stage: "first model response",
            finalizing: false,
        }
    }
    pub(crate) fn at(&self) -> Instant {
        self.deadline
    }
    pub(crate) fn observe(&mut self, event: &RelayEvent) {
        match event {
            RelayEvent::Delta(_) if !self.finalizing => {
                self.deadline = Instant::now() + Duration::from_secs(90);
                self.stage = "model progress";
            }
            RelayEvent::Finalizing(_) | RelayEvent::MessageCompleted(_) if !self.finalizing => {
                self.finalizing = true;
                self.deadline = Instant::now() + Duration::from_secs(60);
                self.stage = "final response receipt";
            }
            _ => {}
        }
    }
    pub(crate) fn error(&self) -> crate::SecureClientError {
        crate::SecureClientError::with_code(
            axiom_inference::ProviderFailureKind::Transient,
            crate::SecureErrorCode::StreamTimeout,
            match self.stage {
                "first model response" => "Timed out waiting for the model to start responding.",
                "final response receipt" => "Timed out waiting for the final response receipt.",
                _ => "The provider stopped responding.",
            },
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::dto::{CreatedPayload, DeltaPayload, RunPayload};

    #[tokio::test(start_paused = true)]
    async fn progressing_stream_outlives_two_minutes_but_metadata_cannot_extend_it() {
        let mut deadline = StreamDeadline::new();
        let first = deadline.at();
        tokio::time::advance(Duration::from_secs(250)).await;
        deadline.observe(&RelayEvent::RunCreated(CreatedPayload {
            run_id: "r".into(),
            inference_encryption: "e2ee".into(),
        }));
        assert_eq!(deadline.at(), first);
        for sequence in 0..10 {
            let delta: DeltaPayload =
                serde_json::from_value(serde_json::json!({"run_id":"r", "sequence":sequence}))
                    .unwrap();
            deadline.observe(&RelayEvent::Delta(delta));
            tokio::time::advance(Duration::from_secs(60)).await;
            assert!(Instant::now() < deadline.at());
        }
        tokio::time::advance(Duration::from_secs(31)).await;
        assert!(Instant::now() > deadline.at());
    }

    #[tokio::test(start_paused = true)]
    async fn receipt_deadline_is_bounded_even_with_duplicate_finalizing_events() {
        let mut deadline = StreamDeadline::new();
        deadline.observe(&RelayEvent::Finalizing(RunPayload { run_id: "r".into() }));
        let receipt = deadline.at();
        tokio::time::advance(Duration::from_secs(59)).await;
        deadline.observe(&RelayEvent::Finalizing(RunPayload { run_id: "r".into() }));
        assert_eq!(receipt, deadline.at());
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(Instant::now() > deadline.at());
    }
}
