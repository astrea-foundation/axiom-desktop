//! Finite native billing controls. Credentials and provider bodies stay in the native client.

use super::*;
use axiom_acp_extension::{
    AutomaticTopup, BillingOperation, BillingPortal, CardFundingOptions, CardFundingPayment,
    CreditBillingAction as Action, CreditBillingResult as Output, CreditPrices, CreditSummary,
    CreditUsage, SavedBillingCards, SubscriptionPlans,
};
use serde_json::json;

fn invalid() -> AxiomError {
    AxiomError::Protocol("Axiom returned invalid credit billing data".into())
}

pub fn credit_integer(value: &str, signed: bool) -> Result<i128> {
    let digits = value.strip_prefix('-').filter(|_| signed).unwrap_or(value);
    if digits.is_empty()
        || digits.len() > 16
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
        || value == "-0"
    {
        return Err(invalid());
    }
    let amount = value.parse::<i128>().map_err(|_| invalid())?;
    if amount.abs() > 9_000_000_000_000_000 || (!signed && amount < 0) {
        return Err(invalid());
    }
    Ok(amount)
}

#[must_use]
pub fn valid_billing_url(value: &str) -> bool {
    if value.len() > 4096 || value.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && matches!((url.host_str(), url.path()),
                (Some("checkout.stripe.com"), path) if path.starts_with("/c/pay/"))
            || url.scheme() == "https"
                && url.username().is_empty()
                && url.password().is_none()
                && url.port().is_none()
                && url.host_str() == Some("billing.stripe.com")
                && url.path().starts_with("/p/session/")
    })
}

fn uuid(value: &str) -> Result<()> {
    let parsed = ::uuid::Uuid::parse_str(value).map_err(|_| invalid())?;
    if parsed.get_version_num() != 4 || parsed.to_string() != value {
        return Err(invalid());
    }
    Ok(())
}

fn date(value: Option<&str>) -> Result<()> {
    if value.is_some_and(|v| DateTime::parse_from_rfc3339(v).is_err()) {
        return Err(invalid());
    }
    Ok(())
}

pub fn validate_credit_summary(summary: &CreditSummary) -> Result<()> {
    if summary.format != "credits@3"
        || summary.scale != "1000000"
        || !summary.enabled
        || !(1..=1_000_000).contains(&summary.reference_microusd_per_credit)
        || summary.purchased.currency != "usd"
        || summary.purchased.scale != "100000000"
        || !(1..=100_000_000).contains(&summary.default_multiplier_ppm)
        || summary.policy_version.is_empty()
        || summary.policy_version.len() > 80
    {
        return Err(invalid());
    }
    credit_integer(&summary.revision, false)?;
    let posted = credit_integer(&summary.purchased.posted_units, true)?;
    let held = credit_integer(&summary.purchased.held_units, false)?;
    if credit_integer(&summary.purchased.available_units, false)? != (posted - held).max(0) {
        return Err(invalid());
    }
    let remaining = credit_integer(&summary.allowance.remaining_units, false)?;
    let held = credit_integer(&summary.allowance.held_units, false)?;
    if remaining + held > credit_integer(&summary.allowance.limit_units, false)?
        || credit_integer(&summary.trial.held_units, false)?
            > credit_integer(&summary.trial.posted_units, false)?
    {
        return Err(invalid());
    }
    date(summary.allowance.resets_at.as_deref())?;
    date(summary.allowance.effective_until.as_deref())?;
    if let Some(subscription) = &summary.subscription {
        uuid(&subscription.id)?;
        if subscription.amount_cents == 0
            || subscription.amount_cents > 1_000_000
            || credit_integer(&subscription.weekly_units, false)? == 0
            || ![
                "creating", "pending", "active", "past_due", "canceled", "expired", "review",
            ]
            .contains(&subscription.state.as_str())
        {
            return Err(invalid());
        }
        date(subscription.paid_through_at.as_deref())?;
    }
    Ok(())
}

fn validate_operation(value: &BillingOperation) -> Result<()> {
    uuid(&value.id)?;
    if value.amount_cents > 1_000_000
        || DateTime::parse_from_rfc3339(&value.created_at).is_err()
        || value
            .url
            .as_deref()
            .is_some_and(|url| !valid_billing_url(url))
        || ![
            "creating",
            "pending",
            "active",
            "past_due",
            "canceled",
            "expired",
            "review",
            "succeeded",
            "failed",
            "requires_action",
            "refunded",
            "partially_refunded",
        ]
        .contains(&value.state.as_str())
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_card_payment(value: &CardFundingPayment) -> Result<()> {
    uuid(&value.id)?;
    if value.amount_microusd < 5_000_000
        || value.amount_microusd > 1_000_000_000
        || !value.amount_microusd.is_multiple_of(10_000)
        || value.credited_microusd > value.amount_microusd
        || value.refunded_microusd > value.amount_microusd
        || ![
            "creating", "open", "pending", "credited", "expired", "refunded", "review",
        ]
        .contains(&value.status.as_str())
        || value
            .checkout_url
            .as_deref()
            .is_some_and(|url| !valid_billing_url(url))
        || DateTime::parse_from_rfc3339(&value.expires_at).is_err()
    {
        return Err(invalid());
    }
    Ok(())
}

impl BillingClient {
    pub async fn credit_control(
        &self,
        action: &Action,
        cancellation: &CancellationToken,
    ) -> Result<Output> {
        let (method, path, body) = match action {
            Action::Summary => (reqwest::Method::GET, "/summary".into(), None),
            Action::Plans => (reqwest::Method::GET, "/plans".into(), None),
            Action::Prices => (reqwest::Method::GET, "/credits/prices".into(), None),
            Action::Usage { period, timezone } => {
                let period = period.as_deref().unwrap_or("all_time");
                let timezone = timezone.as_deref().unwrap_or("UTC");
                if !["week", "month", "all_time"].contains(&period)
                    || timezone.is_empty()
                    || timezone.len() > 128
                    || timezone.chars().any(char::is_control)
                {
                    return Err(invalid());
                }
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair("period", period)
                    .append_pair("timezone", timezone)
                    .finish();
                (
                    reqwest::Method::GET,
                    format!("/credits/usage?{query}"),
                    None,
                )
            }
            Action::Checkout {
                id,
                plan_key,
                catalog_version,
            }
            | Action::Change {
                id,
                plan_key,
                catalog_version,
            } => {
                uuid(id)?;
                if !["starter", "standard", "plus", "max"].contains(&plan_key.as_str()) {
                    return Err(invalid());
                }
                if catalog_version.is_empty()
                    || catalog_version.len() > 80
                    || catalog_version.chars().any(char::is_control)
                {
                    return Err(invalid());
                }
                (
                    reqwest::Method::POST,
                    if matches!(action, Action::Checkout { .. }) {
                        "/subscriptions/checkout"
                    } else {
                        "/subscriptions/change"
                    }
                    .into(),
                    Some(json!({"id":id,"plan_key":plan_key,"catalog_version":catalog_version})),
                )
            }
            Action::Portal => (
                reqwest::Method::POST,
                "/subscriptions/portal".into(),
                Some(json!({})),
            ),
            Action::SetupCard { id } => {
                uuid(id)?;
                (
                    reqwest::Method::POST,
                    "/payment-methods/setup".into(),
                    Some(json!({"id":id})),
                )
            }
            Action::Cards => (reqwest::Method::GET, "/payment-methods".into(), None),
            Action::AutoTopup => (reqwest::Method::GET, "/auto-topup".into(), None),
            Action::ConfigureAutoTopup { settings } => {
                credit_integer(&settings.version, false)?;
                if credit_integer(&settings.threshold_units, false)? <= 0
                    || !(500..=100_000).contains(&settings.amount_cents)
                    || settings.monthly_cap_cents > 1_000_000
                    || (settings.enabled
                        && (!settings.consent_accepted
                            || settings.consent_version != "auto-topup-v2"
                            || settings.monthly_cap_cents < settings.amount_cents
                            || settings.payment_method_id.as_deref().is_none_or(|id| {
                                !id.starts_with("pm_")
                                    || id.len() > 255
                                    || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                            })))
                {
                    return Err(invalid());
                }
                (
                    reqwest::Method::PATCH,
                    "/auto-topup".into(),
                    Some(serde_json::to_value(settings).map_err(|_| invalid())?),
                )
            }
            Action::Fallback { enabled } => (
                reqwest::Method::POST,
                "/credits/fallback".into(),
                Some(json!({"enabled":enabled})),
            ),
            Action::Operations => (reqwest::Method::GET, "/operations".into(), None),
            Action::Operation { id } => {
                uuid(id)?;
                (reqwest::Method::GET, format!("/operations/{id}"), None)
            }
            Action::Check { id } => {
                uuid(id)?;
                (
                    reqwest::Method::POST,
                    format!("/operations/{id}/check"),
                    Some(json!({})),
                )
            }
            Action::CancelTopup { id } => {
                uuid(id)?;
                (
                    reqwest::Method::POST,
                    format!("/auto-topup/{id}/cancel"),
                    Some(json!({})),
                )
            }
            Action::CardOptions => (reqwest::Method::GET, "/stripe/options".into(), None),
            Action::CardPayments => (reqwest::Method::GET, "/stripe/payments".into(), None),
            Action::CreateCardPayment {
                id,
                amount_microusd,
            } => {
                uuid(id)?;
                if !(5_000_000..=1_000_000_000).contains(amount_microusd)
                    || !amount_microusd.is_multiple_of(10_000)
                {
                    return Err(invalid());
                }
                (
                    reqwest::Method::POST,
                    "/stripe/payments".into(),
                    Some(json!({"id":id,"amount_microusd":amount_microusd})),
                )
            }
            Action::CheckCardPayment { id } | Action::CancelCardPayment { id } => {
                uuid(id)?;
                let suffix = if matches!(action, Action::CheckCardPayment { .. }) {
                    "check"
                } else {
                    "cancel"
                };
                (
                    reqwest::Method::POST,
                    format!("/stripe/payments/{id}/{suffix}"),
                    Some(json!({})),
                )
            }
        };
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let mut request = self
            .client
            .request(method, self.endpoint(&format!("/api/v1/billing{path}")))
            .bearer_auth(lease.expose_for_authorization());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = send_with_cancellation(request, cancellation, "credit billing").await?;
        if response.status() == StatusCode::NOT_FOUND && matches!(action, Action::Summary) {
            self.ensure_account_current(&lease)?;
            return Ok(Output::Summary(None));
        }
        if response.status() == StatusCode::NOT_FOUND && matches!(action, Action::CardOptions) {
            self.ensure_account_current(&lease)?;
            return Ok(Output::CardOptions(CardFundingOptions {
                enabled: false,
                currency: "usd".into(),
                min_amount_microusd: 5_000_000,
                max_amount_microusd: 1_000_000_000,
            }));
        }
        ensure_success(response.status(), "credit billing")?;
        let result = match action {
            Action::Summary | Action::Fallback { .. } => {
                let value =
                    decode_bounded_json::<CreditSummary>(response, cancellation, "credit summary")
                        .await?;
                validate_credit_summary(&value)?;
                Output::Summary(Some(Box::new(value)))
            }
            Action::Plans => {
                let value = decode_bounded_json::<SubscriptionPlans>(
                    response,
                    cancellation,
                    "subscription plans",
                )
                .await?;
                if value.currency != "usd"
                    || value.billing_interval != "month"
                    || value.allowance_interval_days != 7
                    || value.plans.len() > 20
                {
                    return Err(invalid());
                }
                for plan in &value.plans {
                    credit_integer(&plan.weekly_units, false)?;
                    if plan.amount_cents > 1_000_000 {
                        return Err(invalid());
                    }
                }
                Output::Plans(value)
            }
            Action::Prices => {
                let value =
                    decode_bounded_json::<CreditPrices>(response, cancellation, "credit prices")
                        .await?;
                if value.scale != "1000000"
                    || !(1..=1_000_000).contains(&value.reference_microusd_per_credit)
                    || !(1..=100_000_000).contains(&value.default_multiplier_ppm)
                    || value.overrides.len() > 512
                    || value
                        .overrides
                        .iter()
                        .any(|p| !(1..=100_000_000).contains(&p.multiplier_ppm))
                {
                    return Err(invalid());
                }
                Output::Prices(value)
            }
            Action::Portal => {
                let value =
                    decode_bounded_json::<BillingPortal>(response, cancellation, "billing portal")
                        .await?;
                if !valid_billing_url(&value.url) {
                    return Err(invalid());
                }
                Output::Portal(value)
            }
            Action::AutoTopup | Action::ConfigureAutoTopup { .. } => {
                let value =
                    decode_bounded_json::<AutomaticTopup>(response, cancellation, "auto topup")
                        .await?;
                credit_integer(&value.version, false)?;
                credit_integer(&value.threshold_units, false)?;
                if !(500..=100_000).contains(&value.amount_cents)
                    || value.monthly_cap_cents > 1_000_000
                    || value.consent_version != "auto-topup-v2"
                    || value.currency != "usd"
                    || value.scale != "100000000"
                {
                    return Err(invalid());
                }
                Output::AutoTopup(value)
            }
            Action::Cards => {
                let value =
                    decode_bounded_json::<SavedBillingCards>(response, cancellation, "saved cards")
                        .await?;
                if value.cards.len() > 20
                    || value.cards.iter().any(|c| {
                        c.last4.len() != 4
                            || !c.last4.bytes().all(|b| b.is_ascii_digit())
                            || c.exp_month == 0
                            || c.exp_month > 12
                    })
                {
                    return Err(invalid());
                }
                Output::Cards(value)
            }
            Action::Operations => {
                let value = decode_bounded_json::<Vec<BillingOperation>>(
                    response,
                    cancellation,
                    "billing operations",
                )
                .await?;
                if value.len() > 20 {
                    return Err(invalid());
                }
                for op in &value {
                    validate_operation(op)?;
                }
                Output::Operations(value)
            }
            Action::CardOptions => {
                let value = decode_bounded_json::<CardFundingOptions>(
                    response,
                    cancellation,
                    "card options",
                )
                .await?;
                if value.currency != "usd"
                    || value.min_amount_microusd > value.max_amount_microusd
                    || value.max_amount_microusd > 1_000_000_000
                {
                    return Err(invalid());
                }
                Output::CardOptions(value)
            }
            Action::CardPayments => {
                let value = decode_bounded_json::<Vec<CardFundingPayment>>(
                    response,
                    cancellation,
                    "card payments",
                )
                .await?;
                if value.len() > 20 {
                    return Err(invalid());
                }
                for payment in &value {
                    validate_card_payment(payment)?;
                }
                Output::CardPayments(value)
            }
            Action::CreateCardPayment { .. }
            | Action::CheckCardPayment { .. }
            | Action::CancelCardPayment { .. } => {
                let value = decode_bounded_json::<CardFundingPayment>(
                    response,
                    cancellation,
                    "card payment",
                )
                .await?;
                validate_card_payment(&value)?;
                Output::CardPayment(value)
            }
            Action::Usage { period, .. } => {
                let value =
                    decode_bounded_json::<CreditUsage>(response, cancellation, "credit usage")
                        .await?;
                if value.scale != "1000000"
                    || value.models.len() > 512
                    || value.period != period.as_deref().unwrap_or("all_time")
                {
                    return Err(invalid());
                }
                let total = credit_integer(&value.total_units, false)?;
                if credit_integer(&value.allocation.subscription_units, false)?
                    + credit_integer(&value.allocation.trial_units, false)?
                    != total
                {
                    return Err(invalid());
                }
                let mut sum = 0_i128;
                for model in &value.models {
                    sum += credit_integer(&model.units, false)?;
                }
                if sum != total {
                    return Err(invalid());
                }
                if value.purchased.currency != "usd"
                    || value.purchased.scale != "100000000"
                    || value.purchased.models.len() > 512
                {
                    return Err(invalid());
                }
                let money_total = credit_integer(&value.purchased.total_units, false)?;
                let money_models =
                    value
                        .purchased
                        .models
                        .iter()
                        .try_fold(0_i128, |total, model| {
                            Ok::<_, AxiomError>(total + credit_integer(&model.units, false)?)
                        })?;
                if money_total != money_models {
                    return Err(invalid());
                }
                Output::Usage(value)
            }
            _ => {
                let value = decode_bounded_json::<BillingOperation>(
                    response,
                    cancellation,
                    "billing operation",
                )
                .await?;
                validate_operation(&value)?;
                Output::Operation(value)
            }
        };
        self.ensure_account_current(&lease)?;
        if cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_subscription_summary_decodes_and_validates() {
        let value = json!({
            "format": "credits@3", "enabled": true, "scale": "1000000", "revision": "1",
            "policy_version": "credits-v3", "reference_microusd_per_credit": 10_000,
            "default_multiplier_ppm": 1_000_000, "paid_fallback": true,
            "payment_review_required": false,
            "purchased": {"currency": "usd", "scale": "100000000", "posted_units": "100000000",
                "held_units": "25000000", "available_units": "75000000"},
            "trial": {"posted_units": "0", "held_units": "0"},
            "allowance": {"limit_units": "125000000", "remaining_units": "125000000",
                "held_units": "0", "generation": "1", "resets_at": "2026-11-20T00:00:00Z",
                "effective_until": "2026-12-13T00:00:00Z"},
            "subscription": {"id": "20000000-0000-4000-8000-000000000001", "plan_key": "starter",
                "amount_cents": 250, "weekly_units": "125000000", "state": "active",
                "paid_through_at": "2026-12-13T00:00:00Z", "cancel_at_period_end": false,
                "pending_plan_key": null},
            "capabilities": {"subscriptions": true, "auto_topup": true, "card": true,
                "crypto": false, "admin": false}
        });
        let summary: CreditSummary = serde_json::from_value(value.clone()).unwrap();
        validate_credit_summary(&summary).unwrap();
        let subscription = summary.subscription.unwrap();
        assert_eq!(subscription.amount_cents, 250);
        assert_eq!(subscription.weekly_units, "125000000");
        for (field, invalid_value) in [
            ("amount_cents", json!(0)),
            ("amount_cents", json!(1_000_001)),
            ("weekly_units", json!("0")),
            ("weekly_units", json!("1e6")),
        ] {
            let mut invalid = value.clone();
            invalid["subscription"][field] = invalid_value;
            let summary: CreditSummary = serde_json::from_value(invalid).unwrap();
            assert!(validate_credit_summary(&summary).is_err());
        }
        let mut unexpected = value;
        unexpected["subscription"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<CreditSummary>(unexpected).is_err());
    }

    #[test]
    fn credit_strings_are_exact_and_bounded() {
        assert_eq!(credit_integer("120000", false).unwrap(), 120_000);
        for bad in ["1e6", "01", "-0", "-1", "9000000000000001", "1.2"] {
            assert!(credit_integer(bad, false).is_err());
        }
        assert_eq!(credit_integer("-120000", true).unwrap(), -120_000);
    }
    #[test]
    fn hosted_destinations_are_narrow() {
        assert!(valid_billing_url(
            "https://checkout.stripe.com/c/pay/example"
        ));
        assert!(valid_billing_url(
            "https://billing.stripe.com/p/session/example"
        ));
        for bad in [
            "https://billing.stripe.com.example.com/p/session/x",
            "https://user@billing.stripe.com/p/session/x",
            "javascript:alert(1)",
            "https://billing.stripe.com/wrong",
        ] {
            assert!(!valid_billing_url(bad));
        }
    }
}
