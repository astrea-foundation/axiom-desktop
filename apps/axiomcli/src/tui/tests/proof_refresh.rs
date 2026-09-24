use super::*;
use crate::tui::proof_refresh::{PROOF_IDLE_AFTER, ProofRefresh};
use std::time::{Duration, Instant};

fn verified_state(now: Instant) -> TuiState {
    let mut state = TuiState::new(
        PathBuf::from("/tmp/project"),
        "deepseek-v4-flash".into(),
        PermissionProfile::Confirm,
    );
    let mut evidence = security_evidence();
    evidence.verified_at_unix_seconds = 1_000;
    evidence.hard_expires_at_unix_seconds = Some(1_300);
    state.security = "SECURE";
    state.security_evidence = Some(evidence);
    state.proof_refresh = ProofRefresh::new(now);
    state.proof_refresh.enable();
    state
}

#[test]
fn composition_reuses_proof_until_expiry_and_never_extends_evidence() {
    let now = Instant::now();
    let mut state = verified_state(now);
    assert!(!state.security_refresh_due(now, 1_239));
    assert!(!state.security_refresh_due(now, 1_240));
    assert!(!state.security_refresh_due(now, 1_299));
    assert!(state.security_refresh_due(now, 1_300));
    assert_eq!(
        state
            .security_evidence
            .as_ref()
            .unwrap()
            .hard_expires_at_unix_seconds,
        Some(1_300)
    );
    state.model = "another-model".into();
    assert!(
        state.security_refresh_due(now, 1_010),
        "a previous model's report is not reused"
    );
    state.security_evidence = None;
    assert!(state.security_refresh_due(now, 1_010));
}

#[test]
fn renewal_waits_for_running_work_and_in_flight_verification() {
    let now = Instant::now();
    let mut state = verified_state(now);
    state.running = true;
    assert!(!state.security_refresh_due(now, 1_300));
    state.running = false;
    state.compacting = true;
    assert!(!state.security_refresh_due(now, 1_300));
    state.compacting = false;
    state.security = "VERIFYING";
    assert!(!state.security_refresh_due(now, 1_300));
    state.security = "SECURE";
    assert!(state.security_refresh_due(now, 1_300));
}

#[test]
fn background_refreshes_do_not_keep_an_inactive_tui_awake() {
    let now = Instant::now();
    let mut refresh = ProofRefresh::new(now);
    assert!(
        !refresh.due(now),
        "only an authenticated preflight enables renewal"
    );
    refresh.enable();
    refresh.completed(now + Duration::from_secs(29 * 60), true);
    assert!(refresh.due(now + Duration::from_secs(30 * 60 - 1)));
    assert!(refresh.is_idle(now + PROOF_IDLE_AFTER));
    assert!(!refresh.due(now + PROOF_IDLE_AFTER));
    let wake = now + PROOF_IDLE_AFTER + Duration::from_secs(1);
    refresh.record_activity(wake);
    assert!(!refresh.is_idle(wake));
    assert!(refresh.due(wake));
}

#[test]
fn idle_header_is_neutral_and_waking_never_verifies_expired_evidence() {
    let now = Instant::now();
    let mut state = verified_state(now.checked_sub(PROOF_IDLE_AFTER).unwrap());
    state.options.animation = false;
    assert_eq!(state.report_status(), ReportStatus::Idle);
    let rendered = render_symbols(100, 30, &state);
    assert!(rendered.contains("IDLE"));
    assert!(!rendered.contains("PROOF EXPIRED"));
    assert!(!rendered.contains("TEE VERIFIED"));
    assert!(!state.security_refresh_due(now, 5_000));
    state.proof_refresh.record_activity(now);
    assert_eq!(state.report_status(), ReportStatus::Expired);
    assert!(state.security_refresh_due(now, 5_000));
}

#[test]
fn composition_retries_are_bounded_and_even_successful_short_leases_are_paced() {
    let mut now = Instant::now();
    let mut refresh = ProofRefresh::new(now);
    refresh.enable();
    for delay in [30, 60, 120, 240, 300, 300] {
        refresh.record_activity(now);
        refresh.completed(now, false);
        assert!(!refresh.due(now + Duration::from_secs(delay - 1)));
        now += Duration::from_secs(delay);
        assert!(refresh.due(now));
    }
    refresh.completed(now, true);
    assert!(!refresh.due(now + Duration::from_secs(29)));
    assert!(refresh.due(now + Duration::from_secs(30)));
}

#[test]
fn resetting_security_clears_idle_state_and_disables_automatic_checks() {
    let now = Instant::now();
    let mut state = verified_state(now.checked_sub(PROOF_IDLE_AFTER).unwrap());
    state.reset_security();
    assert_eq!(state.report_status(), ReportStatus::Unverified);
    assert!(!state.security_refresh_due(now, 5_000));
    assert!(state.security_evidence.is_none());
    state.proof_refresh.enable();
    assert!(state.security_refresh_due(Instant::now(), 5_000));
}
