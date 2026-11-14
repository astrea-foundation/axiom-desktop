//! Explicit account billing commands. Operation IDs make retries deliberate and safe.

use axiom_acp_extension::{
    AutomaticTopupSettings, CreditBillingAction as Action, CreditBillingResult as Output,
};
use clap::{Args, Subcommand};
use tokio_util::sync::CancellationToken;

use super::BillingClient;
use crate::{Result, auth::AuthManager};

fn usd_threshold_units(value: &str) -> Result<String> {
    let mut parts = value.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next().unwrap_or("");
    if whole.is_empty()
        || whole.len() > 8
        || fraction.len() > 8
        || parts.next().is_some()
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(crate::AxiomError::Protocol(
            "Enter a positive USD threshold with up to eight decimal places".into(),
        ));
    }
    let units = whole.parse::<u128>().unwrap_or(0) * 100_000_000
        + fraction.parse::<u128>().unwrap_or(0)
            * 10_u128.pow(u32::try_from(8 - fraction.len()).unwrap_or(0));
    let value = units.to_string();
    if super::credits::credit_integer(&value, false)? == 0 {
        return Err(crate::AxiomError::Protocol(
            "USD threshold must be positive".into(),
        ));
    }
    Ok(value)
}

#[derive(Debug, Args)]
pub struct Arguments {
    #[command(subcommand)]
    action: BillingCommand,
}

#[derive(Debug, Subcommand)]
enum BillingCommand {
    /// Show weekly, purchased and trial credits separately.
    Status,
    Plans,
    /// Show the default and per-model credit multipliers applied to new requests.
    Prices,
    /// Exact fractional credit consumption by model.
    Usage {
        #[arg(long, value_parser = ["week", "month", "all_time"], default_value = "all_time")]
        period: String,
        #[arg(long, default_value = "UTC")]
        timezone: String,
    },
    /// Create subscription Checkout. Reuse the same UUID v4 after an uncertain response.
    Subscribe {
        #[arg(long)]
        id: String,
        #[arg(long)]
        plan: String,
        /// Version returned by `billing plans` for the displayed price/allowance.
        #[arg(long)]
        catalog_version: String,
    },
    /// Schedule a plan change for the next monthly renewal.
    ChangePlan {
        #[arg(long)]
        id: String,
        #[arg(long)]
        plan: String,
        #[arg(long)]
        catalog_version: String,
    },
    /// Get the hosted cancellation and payment-method management URL.
    Portal,
    /// Save a card for off-session use; this does not enable automatic top-up.
    SaveCard {
        #[arg(long)]
        id: String,
    },
    Cards,
    /// Create one card payment. The JSON response contains the hosted Checkout URL.
    TopUp {
        #[arg(long)]
        id: String,
        #[arg(long)]
        amount_cents: u64,
    },
    CardPayments,
    CheckCardPayment {
        #[arg(long)]
        id: String,
    },
    CancelCardPayment {
        #[arg(long)]
        id: String,
    },
    AutoTopup,
    /// Explicit consent to charge a saved card within a monthly UTC cap.
    EnableAutoTopup {
        /// Purchased dollar balance below which automatic refill becomes eligible.
        #[arg(long)]
        threshold_usd: String,
        #[arg(long)]
        amount_cents: u64,
        #[arg(long)]
        monthly_cap_cents: u64,
        #[arg(long)]
        payment_method: String,
        /// Authorize the specified off-session card charges.
        #[arg(long)]
        consent: bool,
    },
    DisableAutoTopup,
    /// Permit or stop spending purchased credits after weekly/trial credits.
    Fallback {
        #[arg(value_parser = ["on", "off"])]
        setting: String,
    },
    Operations,
    Operation {
        #[arg(long)]
        id: String,
    },
    Check {
        #[arg(long)]
        id: String,
    },
    CancelTopup {
        #[arg(long)]
        id: String,
    },
}

pub async fn run(arguments: Arguments, auth: &AuthManager) -> Result<()> {
    let client = BillingClient::new(
        auth.account_api_origin().as_str(),
        auth.clone(),
        std::time::Duration::from_secs(20),
    )?;
    let cancellation = CancellationToken::new();
    let action = match arguments.action {
        BillingCommand::Status => Action::Summary,
        BillingCommand::Plans => Action::Plans,
        BillingCommand::Prices => Action::Prices,
        BillingCommand::Usage { period, timezone } => Action::Usage {
            period: Some(period),
            timezone: Some(timezone),
        },
        BillingCommand::Subscribe {
            id,
            plan,
            catalog_version,
        } => Action::Checkout {
            id,
            plan_key: plan,
            catalog_version,
        },
        BillingCommand::ChangePlan {
            id,
            plan,
            catalog_version,
        } => Action::Change {
            id,
            plan_key: plan,
            catalog_version,
        },
        BillingCommand::Portal => Action::Portal,
        BillingCommand::SaveCard { id } => Action::SetupCard { id },
        BillingCommand::Cards => Action::Cards,
        BillingCommand::TopUp { id, amount_cents } => Action::CreateCardPayment {
            id,
            amount_microusd: amount_cents
                .checked_mul(10_000)
                .ok_or_else(|| crate::AxiomError::Protocol("Invalid top-up amount".into()))?,
        },
        BillingCommand::CardPayments => Action::CardPayments,
        BillingCommand::CheckCardPayment { id } => Action::CheckCardPayment { id },
        BillingCommand::CancelCardPayment { id } => Action::CancelCardPayment { id },
        BillingCommand::AutoTopup => Action::AutoTopup,
        BillingCommand::EnableAutoTopup {
            threshold_usd,
            amount_cents,
            monthly_cap_cents,
            payment_method,
            consent,
        } => {
            if !consent {
                return Err(crate::AxiomError::Protocol(
                    "Enabling automatic charges requires --consent".into(),
                ));
            }
            let Output::AutoTopup(current) = client
                .credit_control(&Action::AutoTopup, &cancellation)
                .await?
            else {
                unreachable!()
            };
            Action::ConfigureAutoTopup {
                settings: AutomaticTopupSettings {
                    enabled: true,
                    version: current.version,
                    threshold_units: usd_threshold_units(&threshold_usd)?,
                    amount_cents,
                    monthly_cap_cents,
                    payment_method_id: Some(payment_method),
                    consent_version: current.consent_version,
                    consent_accepted: true,
                },
            }
        }
        BillingCommand::DisableAutoTopup => {
            let Output::AutoTopup(current) = client
                .credit_control(&Action::AutoTopup, &cancellation)
                .await?
            else {
                unreachable!()
            };
            Action::ConfigureAutoTopup {
                settings: AutomaticTopupSettings {
                    enabled: false,
                    version: current.version,
                    threshold_units: current.threshold_units,
                    amount_cents: current.amount_cents,
                    monthly_cap_cents: current.monthly_cap_cents,
                    payment_method_id: current.payment_method_id,
                    consent_version: current.consent_version,
                    consent_accepted: false,
                },
            }
        }
        BillingCommand::Fallback { setting } => Action::Fallback {
            enabled: setting == "on",
        },
        BillingCommand::Operations => Action::Operations,
        BillingCommand::Operation { id } => Action::Operation { id },
        BillingCommand::Check { id } => Action::Check { id },
        BillingCommand::CancelTopup { id } => Action::CancelTopup { id },
    };
    let output = client.credit_control(&action, &cancellation).await?;
    let json = serde_json::to_string_pretty(&output)
        .map_err(|_| crate::AxiomError::Protocol("Could not format billing result".into()))?;
    println!("{json}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::usd_threshold_units;

    #[test]
    fn automatic_refill_thresholds_are_usd_amounts() {
        assert_eq!(usd_threshold_units("1.25").unwrap(), "125000000");
        assert_eq!(usd_threshold_units("100.25").unwrap(), "10025000000");
        assert_eq!(usd_threshold_units("0.00000001").unwrap(), "1");
        for invalid in ["0", "-1", "1e3", "1.000000001", "90000001"] {
            assert!(usd_threshold_units(invalid).is_err());
        }
    }
}
