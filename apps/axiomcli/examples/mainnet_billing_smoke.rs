//! Exercise the desktop's real native-account billing path without opening a GUI.
//! Never prints credentials, customer addresses, or conversation content.
use axiomcli::{
    auth::{AuthManager, ValidationStatus},
    billing::BillingClient,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let origin = std::env::var("AXIOM_BASE_URL")?;
    let auth = AuthManager::new(&origin, Duration::from_secs(20))?;
    anyhow::ensure!(
        matches!(auth.validate().await, ValidationStatus::Valid(_)),
        "Sign in to this Axiom environment first"
    );
    let billing = BillingClient::new(&origin, auth, Duration::from_secs(20))?;
    let cancel = CancellationToken::new();
    let mut status = billing.status(&cancel).await?;
    for _ in 0..10 {
        if status
            .payment_account
            .as_ref()
            .is_some_and(|p| p.state == "ready")
        {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        status = billing.status(&cancel).await?;
    }
    let payment = status
        .payment_account
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing payment account"))?;
    anyhow::ensure!(
        payment.network == "mainnet" && payment.state == "ready",
        "Mainnet address not ready"
    );
    anyhow::ensure!(payment.monitoring_status == "ready", "Scanner not ready");
    if std::env::args().any(|arg| arg == "--valuation") {
        anyhow::ensure!(
            payment.valuation_enabled,
            "Deposit valuation is not enabled"
        );
    }
    if std::env::args().any(|arg| arg == "--quote") {
        anyhow::ensure!(
            status.zec_usd_quote.is_some(),
            "A fresh ZEC/USD quote is unavailable"
        );
    }
    let repeated = billing.status(&cancel).await?;
    anyhow::ensure!(
        repeated.payment_account.as_ref().unwrap().address == payment.address,
        "Persistent address changed"
    );
    anyhow::ensure!(
        i64::try_from(status.trial_microusd)?.checked_add(status.paid_microusd)
            == Some(status.posted_microusd),
        "Inconsistent source-separated credit"
    );
    println!(
        "{}",
        serde_json::json!({"native_auth":true, "network":payment.network,
        "address_ready":true,"persistent_address":true,"monitoring_status":payment.monitoring_status,
        "posted_microusd":status.posted_microusd,"trial_microusd":status.trial_microusd,
        "other_credit_microusd":status.paid_microusd,"confirmed_zatoshis":payment.confirmed_zatoshis,
        "zec_usd_quote":status.zec_usd_quote,
        "conversion_status":payment.conversion_status,"credentials_printed":false,"funds_sent":false})
    );
    Ok(())
}
