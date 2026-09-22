//! Local, turn-scoped input. Never sent to a backend outside provider E2EE.
use crate::{AxiomError, Result, app::TurnId};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::sync::oneshot;

pub const MAX_STEERING_BYTES: usize = 64 * 1024;
const MAX_PENDING: usize = 32;
const MAX_TURN_INPUT_BYTES: usize = 256 * 1024;

#[derive(Clone)]
pub struct SteeringMessage {
    pub id: String,
    pub text: String,
}
struct Submission {
    text: String,
    applied: bool,
    waiters: Vec<oneshot::Sender<()>>,
}
#[derive(Default)]
struct State {
    closed: bool,
    bytes: usize,
    pending: VecDeque<SteeringMessage>,
    submissions: HashMap<String, Submission>,
}
pub struct TurnSteering {
    turn_id: TurnId,
    state: Mutex<State>,
}

impl TurnSteering {
    #[must_use]
    pub fn new(turn_id: TurnId) -> Self {
        Self {
            turn_id,
            state: Mutex::default(),
        }
    }
    pub fn submit(
        &self,
        expected: &TurnId,
        id: String,
        text: String,
    ) -> Result<oneshot::Receiver<()>> {
        if expected != &self.turn_id
            || id.is_empty()
            || id.len() > 128
            || text.trim().is_empty()
            || text.len() > MAX_STEERING_BYTES
        {
            return Err(AxiomError::InvalidTransition(
                "invalid steering input or stale turn ID".into(),
            ));
        }
        let mut state = self.state.lock().expect("steering lock");
        let (tx, rx) = oneshot::channel();
        if let Some(previous) = state.submissions.get_mut(&id) {
            if previous.text != text {
                return Err(AxiomError::InvalidTransition(
                    "steering ID was reused with different input".into(),
                ));
            }
            if previous.applied {
                let _ = tx.send(());
            } else if previous.waiters.len() < MAX_PENDING {
                previous.waiters.push(tx);
            } else {
                return Err(AxiomError::InvalidTransition(
                    "too many steering retries".into(),
                ));
            }
            return Ok(rx);
        }
        if state.closed {
            return Err(AxiomError::InvalidTransition(
                "turn is no longer accepting steering".into(),
            ));
        }
        if state.pending.len() >= MAX_PENDING
            || state.bytes.saturating_add(text.len()) > MAX_TURN_INPUT_BYTES
        {
            return Err(AxiomError::InvalidTransition(
                "steering inbox is full".into(),
            ));
        }
        state.bytes += text.len();
        state.pending.push_back(SteeringMessage {
            id: id.clone(),
            text: text.clone(),
        });
        state.submissions.insert(
            id,
            Submission {
                text,
                applied: false,
                waiters: vec![tx],
            },
        );
        Ok(rx)
    }
    pub fn has_pending(&self) -> bool {
        !self.state.lock().expect("steering lock").pending.is_empty()
    }
    pub fn take_pending(&self) -> Vec<SteeringMessage> {
        self.state
            .lock()
            .expect("steering lock")
            .pending
            .drain(..)
            .collect()
    }
    /// Called by the adapter only AFTER the message is committed locally.
    pub fn acknowledge(&self, id: &str) {
        if let Some(input) = self
            .state
            .lock()
            .expect("steering lock")
            .submissions
            .get_mut(id)
        {
            input.applied = true;
            for waiter in input.waiters.drain(..) {
                let _ = waiter.send(());
            }
        }
    }
    /// Atomic with submit: a late input either continues this turn or is rejected.
    pub fn finish_if_empty(&self) -> bool {
        let mut state = self.state.lock().expect("steering lock");
        if !state.pending.is_empty() {
            return false;
        }
        state.closed = true;
        true
    }
    pub fn close(&self) {
        let mut state = self.state.lock().expect("steering lock");
        state.closed = true;
        while let Some(input) = state.pending.pop_front() {
            state.submissions.remove(&input.id);
        }
    }
}

pub struct SteeringGuard(pub Option<Arc<TurnSteering>>);
impl Drop for SteeringGuard {
    fn drop(&mut self) {
        if let Some(inbox) = &self.0 {
            inbox.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn acknowledgements_follow_application_and_retries_are_deduplicated() {
        let turn = TurnId::new();
        let inbox = TurnSteering::new(turn.clone());
        let mut first = inbox.submit(&turn, "one".into(), "change".into()).unwrap();
        let second = inbox.submit(&turn, "one".into(), "change".into()).unwrap();
        assert!(first.try_recv().is_err());
        assert!(!inbox.finish_if_empty());
        assert_eq!(inbox.take_pending().len(), 1);
        inbox.acknowledge("one");
        first.await.unwrap();
        second.await.unwrap();
        assert!(inbox.finish_if_empty());
        assert!(inbox.submit(&turn, "late".into(), "late".into()).is_err());
        assert!(
            inbox
                .submit(&TurnId::new(), "one".into(), "change".into())
                .is_err()
        );
        assert!(
            inbox
                .submit(&turn, "one".into(), "different".into())
                .is_err()
        );
    }
    #[tokio::test]
    async fn cancellation_returns_unapplied_input_to_the_caller() {
        let turn = TurnId::new();
        let inbox = TurnSteering::new(turn.clone());
        let pending = inbox.submit(&turn, "one".into(), "change".into()).unwrap();
        inbox.close();
        assert!(pending.await.is_err());
    }
}
