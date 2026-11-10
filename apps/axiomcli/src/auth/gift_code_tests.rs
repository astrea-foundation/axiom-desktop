// Included in auth::tests to reuse its real native sign-in/credential-store harness.
async fn gift_redeem_test(
    State(state): State<Arc<MockState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer access-token")
    );
    assert_eq!(body, json!({"code": format!("AXG{}", "A".repeat(32))}));
    let previous = state.gift_calls.fetch_add(1, Ordering::AcqRel);
    let mode = state.gift_mode.load(Ordering::Acquire);
    if mode == 1 {
        return (
            AxumStatus::BAD_REQUEST,
            Json(json!({"detail": body["code"]})),
        )
            .into_response();
    }
    if mode == 3 {
        state.gift_started.notify_one();
        state.gift_release.notified().await;
    }
    let amount = if mode == 2 { -1 } else { 2_500_000 };
    Json(json!({
        "credited_microusd": amount, "already_redeemed": previous > 0,
        "status": {"posted_microusd": 3_500_000, "available_microusd": 3_500_000,
            "ledger_sequence": 2, "trial_microusd": 1_000_000, "paid_microusd": 2_500_000,
            "payment_review_required": false, "currency": "microUSD",
            "payment_account": null, "zec_usd_quote": null}
    }))
    .into_response()
}

#[tokio::test]
async fn gift_redemption_uses_native_auth_and_validates_receipts_without_echoing_secrets() {
    let (auth, state, store) = manager().await;
    let client = crate::billing::BillingClient::new(
        auth.api_base_url.as_str(),
        auth.clone(),
        Duration::from_secs(2),
    )
    .unwrap();
    let code = format!("AXG-{}", "AAAA-".repeat(8).trim_end_matches('-'));
    let cancellation = CancellationToken::new();
    assert!(client.redeem_gift_code(&code, &cancellation).await.is_err());
    assert_eq!(state.gift_calls.load(Ordering::Acquire), 0);
    let login = auth.start_native_login(None).await.unwrap();
    state.approved.store(true, Ordering::Release);
    login.complete(CancellationToken::new()).await.unwrap();
    let credential = store.load().unwrap();
    let receipt = client
        .redeem_gift_code(&code.to_lowercase(), &cancellation)
        .await
        .unwrap();
    assert_eq!(receipt.credited_microusd, 2_500_000);
    assert_eq!(receipt.status.paid_microusd, 2_500_000);
    assert!(!receipt.already_redeemed);
    assert!(
        client
            .redeem_gift_code(&code, &cancellation)
            .await
            .unwrap()
            .already_redeemed
    );
    let before = state.gift_calls.load(Ordering::Acquire);
    assert!(
        client
            .redeem_gift_code("bad gift", &cancellation)
            .await
            .is_err()
    );
    assert_eq!(state.gift_calls.load(Ordering::Acquire), before);
    state.gift_mode.store(1, Ordering::Release);
    let error = client
        .redeem_gift_code(&code, &cancellation)
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("invalid or unavailable"));
    assert!(!error.contains("AXG"));
    state.gift_mode.store(2, Ordering::Release);
    assert!(client.redeem_gift_code(&code, &cancellation).await.is_err());
    assert_eq!(store.load().unwrap(), credential);
}

#[tokio::test]
async fn gift_redemption_cancels_and_discards_results_after_logout() {
    let (auth, state, _store) = manager().await;
    let login = auth.start_native_login(None).await.unwrap();
    state.approved.store(true, Ordering::Release);
    login.complete(CancellationToken::new()).await.unwrap();
    let client = crate::billing::BillingClient::new(
        auth.api_base_url.as_str(),
        auth.clone(),
        Duration::from_secs(2),
    )
    .unwrap();
    state.gift_mode.store(3, Ordering::Release);
    let code = format!("AXG{}", "A".repeat(32));
    let cancellation = CancellationToken::new();
    let redeem = client.redeem_gift_code(&code, &cancellation);
    tokio::pin!(redeem);
    tokio::select! { biased; _ = &mut redeem => panic!("request must wait"), () = state.gift_started.notified() => {} }
    cancellation.cancel();
    assert!(matches!(redeem.await, Err(AxiomError::Cancelled)));
    state.gift_release.notify_waiters();

    let next_cancel = CancellationToken::new();
    let next = client.redeem_gift_code(&code, &next_cancel);
    tokio::pin!(next);
    tokio::select! { biased; _ = &mut next => panic!("request must wait"), () = state.gift_started.notified() => {} }
    auth.logout_async().await.unwrap();
    state.gift_release.notify_waiters();
    assert!(
        next.await
            .unwrap_err()
            .to_string()
            .contains("account changed")
    );
}
