//! Composition-triggered proof retry pacing. This never renews evidence itself.

use std::time::{Duration, Instant};

pub(super) const PROOF_IDLE_AFTER: Duration = Duration::from_secs(30 * 60);
const MIN_RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(300);

#[derive(Debug)]
pub(super) struct ProofRefresh {
    enabled: bool,
    last_activity: Instant,
    next_attempt: Option<Instant>,
    failures: u32,
}

impl ProofRefresh {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            enabled: false,
            last_activity: now,
            next_attempt: None,
            failures: 0,
        }
    }

    pub(super) fn enable(&mut self) {
        self.enabled = true;
    }

    pub(super) fn record_activity(&mut self, now: Instant) {
        self.last_activity = now;
    }

    pub(super) fn is_idle(&self, now: Instant) -> bool {
        self.enabled && now.saturating_duration_since(self.last_activity) >= PROOF_IDLE_AFTER
    }

    pub(super) fn due(&self, now: Instant) -> bool {
        self.enabled && !self.is_idle(now) && self.next_attempt.is_none_or(|next| now >= next)
    }

    pub(super) fn completed(&mut self, now: Instant, successful: bool) {
        self.failures = if successful {
            0
        } else {
            (self.failures + 1).min(5)
        };
        // Pace even successful checks: the provider can return a short lease.
        let delay = (MIN_RETRY * 2_u32.pow(self.failures.saturating_sub(1))).min(MAX_RETRY);
        self.next_attempt = Some(now + delay);
    }
}
