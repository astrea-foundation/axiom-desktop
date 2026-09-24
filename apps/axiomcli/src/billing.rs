//! Native-account billing and persistent mainnet deposit client.
//!
//! Billing is an account-control API, not an inference transport. It accepts
//! only short native-session access tokens and never falls back to the manual
//! automation key. Remote bodies and credentials are excluded from errors.

use std::time::Duration;

use chrono::DateTime;
use futures::StreamExt as _;
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result, auth::AuthManager};

const MAX_BILLING_RESPONSE_BYTES: usize = 64 * 1024;
// ACP's generated TypeScript contract represents integer fields as IEEE-754
// numbers. Reject projections that could lose precision at that boundary.
const MAX_ACP_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BillingStatus {
    pub posted_microusd: i64,
    pub available_microusd: i64,
    pub ledger_sequence: u64,
    pub trial_microusd: u64,
    pub paid_microusd: i64,
    pub payment_review_required: bool,
    pub currency: String,
    #[serde(default)]
    pub payment_account: Option<axiom_acp_extension::PaymentAccount>,
    #[serde(default)]
    pub zec_usd_quote: Option<axiom_acp_extension::ZecUsdQuote>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GiftCodeRedemption {
    pub credited_microusd: u64,
    pub already_redeemed: bool,
    pub status: BillingStatus,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageSpend {
    period: String,
    total_cost_microusd: String,
    models: Vec<UsageSpendModel>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageSpendModel {
    provider: String,
    model_id: Option<String>,
    model_name: Option<String>,
    cost_microusd: String,
}

fn validated_usage_spend(
    spend: UsageSpend,
    period: axiom_acp_extension::UsagePeriod,
) -> Result<axiom_acp_extension::UsageSummary> {
    let invalid = || AxiomError::Protocol("Axiom returned invalid usage spending".into());
    let amount = |value: &str| -> Result<u128> {
        if value.is_empty()
            || value.len() > 39
            || !value.bytes().all(|c| c.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(invalid());
        }
        value.parse().map_err(|_| invalid())
    };
    if spend.period != period.as_str() || spend.models.len() > 512 {
        return Err(invalid());
    }
    let total = amount(&spend.total_cost_microusd)?;
    let mut sum = 0_u128;
    let mut models = Vec::new();
    for model in spend.models {
        if model.model_id.is_none() {
            return Err(invalid());
        }
        for label in [
            Some(&model.provider),
            model.model_id.as_ref(),
            model.model_name.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if label.trim().is_empty() || label.len() > 512 || label.chars().any(char::is_control) {
                return Err(invalid());
            }
        }
        sum = sum
            .checked_add(amount(&model.cost_microusd)?)
            .ok_or_else(invalid)?;
        models.push(axiom_acp_extension::ModelSpend {
            provider: model.provider,
            model_id: model.model_id,
            model_name: model.model_name,
            cost_microusd: model.cost_microusd,
        });
    }
    if sum != total {
        return Err(invalid());
    }
    Ok(axiom_acp_extension::UsageSummary {
        period: spend.period,
        total_cost_microusd: spend.total_cost_microusd,
        models,
    })
}

#[derive(Clone, Debug)]
pub struct BillingClient {
    auth: AuthManager,
    client: Client,
    api_base_url: Url,
}

impl BillingClient {
    pub fn new(api_base_url: &str, auth: AuthManager, timeout: Duration) -> Result<Self> {
        let api_base_url = parse_api_origin(api_base_url)?;
        let client = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                AxiomError::Config(format!("could not build the billing client: {error}"))
            })?;
        Ok(Self {
            auth,
            client,
            api_base_url,
        })
    }

    pub async fn status(&self, cancellation: &CancellationToken) -> Result<BillingStatus> {
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let response = send_with_cancellation(
            self.client
                .get(self.endpoint("/api/v1/billing/status"))
                .bearer_auth(lease.expose_for_authorization()),
            cancellation,
            "billing status",
        )
        .await?;
        ensure_success(response.status(), "billing status")?;
        let mut status =
            decode_bounded_json::<BillingStatus>(response, cancellation, "billing-status").await?;
        validate_billing_status(&status)?;
        self.ensure_account_current(&lease)?;
        if status.payment_account.is_some() {
            // Provision once per authenticated app account. The backend owns
            // durable operation keys; repeats poll the corresponding address GET.
            // Never send payment-service credentials to the native client.
            let response = send_with_cancellation(
                self.client
                    .post(self.endpoint("/api/v1/billing/deposit-address"))
                    .bearer_auth(lease.expose_for_authorization())
                    .json(&serde_json::json!({})),
                cancellation,
                "deposit address",
            )
            .await?;
            ensure_success(response.status(), "deposit address")?;
            status.payment_account = Some(
                decode_bounded_json::<axiom_acp_extension::PaymentAccount>(
                    response,
                    cancellation,
                    "deposit-address",
                )
                .await?,
            );
            validate_billing_status(&status)?;
            self.ensure_account_current(&lease)?;
        }
        // Expired/invalid indicative data must not hide an otherwise valid balance.
        status.zec_usd_quote = status
            .zec_usd_quote
            .filter(|quote| valid_current_quote(quote, chrono::Utc::now()));
        Ok(status)
    }

    /// The secret is submitted only to the account API, never to inference or activity logs.
    pub async fn redeem_gift_code(
        &self,
        code: &str,
        cancellation: &CancellationToken,
    ) -> Result<GiftCodeRedemption> {
        let normalized = code.trim().replace(['-', ' '], "").to_ascii_uppercase();
        if code.len() > 64
            || !code.is_ascii()
            || normalized.len() != 35
            || !normalized.starts_with("AXG")
            || !normalized[3..]
                .bytes()
                .all(|b| b.is_ascii_uppercase() || (b'2'..=b'7').contains(&b))
        {
            return Err(AxiomError::Config("Enter a valid Axiom gift code.".into()));
        }
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let response = send_with_cancellation(
            self.client
                .post(self.endpoint("/api/v1/billing/gift-codes/redeem"))
                .bearer_auth(lease.expose_for_authorization())
                .json(&serde_json::json!({"code": normalized})),
            cancellation,
            "gift redemption",
        )
        .await?;
        match response.status() {
            StatusCode::BAD_REQUEST => {
                return Err(AxiomError::Provider(
                    "Gift code is invalid or unavailable.".into(),
                ));
            }
            StatusCode::TOO_MANY_REQUESTS => {
                return Err(AxiomError::Provider(
                    "Too many redemption attempts. Wait a few minutes and try again.".into(),
                ));
            }
            StatusCode::NOT_FOUND => {
                return Err(AxiomError::Provider(
                    "Gift redemption is not available on this server yet.".into(),
                ));
            }
            status => ensure_success(status, "gift redemption")?,
        }
        let mut result: GiftCodeRedemption =
            decode_bounded_json(response, cancellation, "gift-redemption").await?;
        if !(10_000..=10_000_000_000).contains(&result.credited_microusd)
            || !result.credited_microusd.is_multiple_of(10_000)
        {
            return Err(AxiomError::Protocol(
                "Axiom returned an invalid gift-credit receipt".into(),
            ));
        }
        validate_billing_status(&result.status)?;
        self.ensure_account_current(&lease)?;
        result.status.zec_usd_quote = result
            .status
            .zec_usd_quote
            .filter(|quote| valid_current_quote(quote, chrono::Utc::now()));
        Ok(result)
    }

    pub async fn usage_summary(
        &self,
        request: &axiom_acp_extension::UsageSummaryRequest,
        cancellation: &CancellationToken,
    ) -> Result<axiom_acp_extension::UsageSummary> {
        let timezone = request.timezone.as_deref().unwrap_or("UTC");
        if timezone.is_empty()
            || timezone.len() > 128
            || !timezone
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"/_+-".contains(&c))
        {
            return Err(AxiomError::Protocol("Invalid usage timezone".into()));
        }
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let response = send_with_cancellation(
            self.client
                .get(self.endpoint("/api/v1/usage/spend"))
                .query(&[("period", request.period.as_str()), ("timezone", timezone)])
                .bearer_auth(lease.expose_for_authorization()),
            cancellation,
            "usage",
        )
        .await?;
        ensure_success(response.status(), "usage")?;
        let spend = decode_bounded_json::<UsageSpend>(response, cancellation, "usage").await?;
        let summary = validated_usage_spend(spend, request.period)?;
        self.ensure_account_current(&lease)?;
        Ok(summary)
    }

    pub async fn api_keys(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<axiom_acp_extension::ApiKeyListResponse> {
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let response = send_with_cancellation(
            self.client
                .get(self.endpoint("/api/v1/auth/api-keys"))
                .query(&[("include_revoked", "true")])
                .bearer_auth(lease.expose_for_authorization()),
            cancellation,
            "API keys",
        )
        .await?;
        ensure_success(response.status(), "API keys")?;
        let result = decode_bounded_json::<axiom_acp_extension::ApiKeyListResponse>(
            response,
            cancellation,
            "API keys",
        )
        .await?;
        for key in &result.keys {
            validate_api_key_record(key)?;
        }
        self.ensure_account_current(&lease)?;
        Ok(result)
    }

    pub async fn create_api_key(
        &self,
        name: &str,
        cancellation: &CancellationToken,
    ) -> Result<axiom_acp_extension::ApiKeyCreatedResponse> {
        validate_api_key_name(name)?;
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let response = send_with_cancellation(
            self.client
                .post(self.endpoint("/api/v1/auth/api-keys"))
                .bearer_auth(lease.expose_for_authorization())
                .json(&serde_json::json!({"name": name})),
            cancellation,
            "API key creation",
        )
        .await?;
        if response.status() == StatusCode::FORBIDDEN {
            return Err(AxiomError::Provider(
                "Sign in again before creating an API key.".into(),
            ));
        }
        ensure_success(response.status(), "API key creation")?;
        let mut value =
            decode_bounded_json::<serde_json::Value>(response, cancellation, "API key creation")
                .await?;
        let token = value
            .as_object_mut()
            .and_then(|object| object.remove("token"))
            .and_then(|value| value.as_str().map(str::to_owned))
            .filter(|token| {
                token.starts_with("axm_")
                    && (20..=512).contains(&token.len())
                    && token
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            })
            .ok_or_else(|| AxiomError::Protocol("invalid API key creation response".into()))?;
        let key = serde_json::from_value::<axiom_acp_extension::ApiKeyRecord>(value)
            .map_err(|_| AxiomError::Protocol("invalid API key creation response".into()))?;
        validate_api_key_record(&key)?;
        self.ensure_account_current(&lease)?;
        Ok(axiom_acp_extension::ApiKeyCreatedResponse { key, token })
    }

    pub async fn revoke_api_key(
        &self,
        id: &str,
        cancellation: &CancellationToken,
    ) -> Result<axiom_acp_extension::ApiKeyRevokeResponse> {
        validate_api_key_id(id)?;
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let response = send_with_cancellation(
            self.client
                .delete(self.endpoint(&format!("/api/v1/auth/api-keys/{id}")))
                .bearer_auth(lease.expose_for_authorization()),
            cancellation,
            "API key revocation",
        )
        .await?;
        ensure_success(response.status(), "API key revocation")?;
        self.ensure_account_current(&lease)?;
        Ok(axiom_acp_extension::ApiKeyRevokeResponse {})
    }

    fn endpoint(&self, path: &str) -> Url {
        self.api_base_url
            .join(path)
            .expect("static billing API path is valid")
    }

    fn ensure_account_current(&self, lease: &crate::auth::AccountAccessTokenLease) -> Result<()> {
        if self.auth.account_access_is_current(lease) {
            Ok(())
        } else {
            Err(AxiomError::InvalidTransition(
                "the Axiom account changed while billing was in progress; discard the stale result"
                    .into(),
            ))
        }
    }
}

fn validate_api_key_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err(AxiomError::Protocol("invalid API key ID".into()));
    }
    Ok(())
}

fn validate_api_key_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.chars().count() > 120 || name.chars().any(char::is_control) {
        return Err(AxiomError::Protocol(
            "API key names must contain 1–120 printable characters".into(),
        ));
    }
    Ok(())
}

fn validate_api_key_record(key: &axiom_acp_extension::ApiKeyRecord) -> Result<()> {
    validate_api_key_id(&key.id)?;
    validate_api_key_name(&key.name)?;
    let invalid = || AxiomError::Protocol("invalid API key usage response".into());
    if key.scopes != ["inference"] {
        return Err(invalid());
    }
    for date in [
        Some(&key.created_at),
        Some(&key.usage_started_at),
        key.last_used_at.as_ref(),
        key.expires_at.as_ref(),
        key.revoked_at.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        DateTime::parse_from_rfc3339(date).map_err(|_| invalid())?;
    }
    let mut counts = Vec::new();
    for value in [
        &key.usage.request_count,
        &key.usage.input_tokens,
        &key.usage.cached_input_tokens,
        &key.usage.output_tokens,
        &key.usage.cost_microusd,
    ] {
        if value.is_empty()
            || value.len() > 39
            || !value.bytes().all(|c| c.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(invalid());
        }
        counts.push(value.parse::<u128>().map_err(|_| invalid())?);
    }
    if counts[2] > counts[1] {
        return Err(invalid());
    }
    Ok(())
}

async fn decode_bounded_json<T: DeserializeOwned>(
    response: Response,
    cancellation: &CancellationToken,
    shape: &str,
) -> Result<T> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BILLING_RESPONSE_BYTES as u64)
    {
        return Err(AxiomError::Protocol(format!(
            "Axiom {shape} response exceeded the size limit"
        )));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::select! {
            biased;
            chunk = stream.next() => chunk,
            () = cancellation.cancelled() => return Err(AxiomError::Cancelled),
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk
            .map_err(|_| AxiomError::Protocol(format!("could not read Axiom {shape} response")))?;
        if bytes.len().saturating_add(chunk.len()) > MAX_BILLING_RESPONSE_BYTES {
            return Err(AxiomError::Protocol(format!(
                "Axiom {shape} response exceeded the size limit"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AxiomError::Protocol(format!("invalid Axiom {shape} response")))
}

async fn send_with_cancellation(
    request: RequestBuilder,
    cancellation: &CancellationToken,
    operation: &str,
) -> Result<Response> {
    let send = request.send();
    tokio::pin!(send);
    tokio::select! {
        biased;
        response = &mut send => response.map_err(|error| {
            AxiomError::Provider(format!("could not reach Axiom {operation}: {error}"))
        }),
        () = cancellation.cancelled() => Err(AxiomError::Cancelled),
    }
}

fn ensure_success(status: StatusCode, operation: &str) -> Result<()> {
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(AxiomError::Provider(
            "native Axiom account authentication is required for billing".into(),
        ));
    }
    Err(AxiomError::Provider(format!(
        "Axiom {operation} returned HTTP {status}"
    )))
}

fn parse_api_origin(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|error| AxiomError::Config(format!("invalid API URL: {error}")))?;
    let loopback_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
    if (url.scheme() != "https" && !loopback_http)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AxiomError::Config(
            "API URL must be an HTTPS origin (or loopback HTTP origin)".into(),
        ));
    }
    Ok(url)
}

fn validate_billing_status(status: &BillingStatus) -> Result<()> {
    if status.trial_microusd > MAX_ACP_SAFE_INTEGER
        || status.paid_microusd.unsigned_abs() > MAX_ACP_SAFE_INTEGER
        || status.trial_microusd > status.posted_microusd.max(0).unsigned_abs()
        || i128::from(status.trial_microusd) + i128::from(status.paid_microusd)
            != i128::from(status.posted_microusd)
    {
        return Err(AxiomError::Protocol(
            "Axiom returned inconsistent credit buckets".into(),
        ));
    }
    if let Some(payment) = &status.payment_account {
        validate_payment_account(payment)?;
    }
    if status.posted_microusd.unsigned_abs() > MAX_ACP_SAFE_INTEGER
        || status.available_microusd.unsigned_abs() > MAX_ACP_SAFE_INTEGER
        || status.ledger_sequence > MAX_ACP_SAFE_INTEGER
        || status.currency != "microUSD"
        || status.available_microusd
            != if status.payment_review_required {
                0
            } else {
                status.posted_microusd
            }
    {
        return Err(AxiomError::Protocol(
            "Axiom returned inconsistent billing status".into(),
        ));
    }
    Ok(())
}

fn exact_integer(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 19
        && (value == "0" || !value.starts_with('0'))
        && value.bytes().all(|c| c.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| i64::try_from(n).is_ok())
}

fn valid_current_quote(
    quote: &axiom_acp_extension::ZecUsdQuote,
    now: DateTime<chrono::Utc>,
) -> bool {
    let (Ok(as_of), Ok(expires)) = (
        DateTime::parse_from_rfc3339(&quote.as_of),
        DateTime::parse_from_rfc3339(&quote.expires_at),
    ) else {
        return false;
    };
    exact_integer(&quote.price_microusd_per_zec)
        && quote
            .price_microusd_per_zec
            .parse::<u64>()
            .is_ok_and(|n| n > 0 && n <= 1_000_000_000_000)
        && matches!(quote.source.as_str(), "coinbase" | "kraken")
        && as_of <= now
        && now < expires
        && expires.signed_duration_since(as_of) == chrono::Duration::seconds(60)
}

fn validate_payment_account(payment: &axiom_acp_extension::PaymentAccount) -> Result<()> {
    let mut valid = payment.network == "mainnet"
        && payment.asset == "ZEC"
        && payment.conversion_status == "none"
        && matches!(
            payment.state.as_str(),
            "unprovisioned" | "provisioning" | "ready"
        )
        && exact_integer(&payment.confirmed_zatoshis)
        && exact_integer(&payment.confirming_zatoshis)
        && payment
            .required_confirmations
            .as_deref()
            .is_none_or(exact_integer)
        && payment.deposits.len() <= 20;
    match (&payment.address, &payment.payment_uri) {
        (Some(address), Some(uri)) => {
            valid &= address.starts_with("u1")
                && (20..=4096).contains(&address.len())
                && address.bytes().all(|b| b.is_ascii_alphanumeric())
                && *uri == format!("zcash:{address}");
        }
        (None, None) => valid &= payment.state != "ready",
        _ => valid = false,
    }
    for deposit in &payment.deposits {
        valid &= deposit.valuation_status.as_deref().is_none_or(|s| {
            matches!(
                s,
                "unpriced" | "pending" | "credited" | "reversed" | "review"
            )
        });
        match (
            &deposit.credit_microusd,
            &deposit.price_microusd_per_zec,
            &deposit.price_source,
            &deposit.priced_at,
        ) {
            (None, None, None, None) => {
                valid &= !matches!(
                    deposit.valuation_status.as_deref(),
                    Some("credited" | "reversed")
                );
            }
            (Some(credit), Some(rate), Some(source), Some(at)) => {
                valid &= exact_integer(credit)
                    && credit
                        .parse::<u64>()
                        .is_ok_and(|n| n <= MAX_ACP_SAFE_INTEGER)
                    && exact_integer(rate)
                    && rate
                        .parse::<u64>()
                        .is_ok_and(|n| n > 0 && n <= 1_000_000_000_000)
                    && matches!(source.as_str(), "coinbase" | "kraken")
                    && DateTime::parse_from_rfc3339(at).is_ok();
                if let (Ok(amount), Ok(price), Ok(credited)) = (
                    deposit.amount_zatoshis.parse::<u128>(),
                    rate.parse::<u128>(),
                    credit.parse::<u128>(),
                ) {
                    valid &= amount.checked_mul(price).map(|n| n / 100_000_000) == Some(credited);
                } else {
                    valid = false;
                }
            }
            _ => valid = false,
        }
        valid &= uuid::Uuid::parse_str(&deposit.id).is_ok()
            && matches!(
                deposit.state.as_str(),
                "confirming" | "confirmed" | "orphaned"
            )
            && exact_integer(&deposit.amount_zatoshis)
            && deposit.amount_zatoshis != "0"
            && deposit
                .amount_zatoshis
                .parse::<u64>()
                .is_ok_and(|n| n <= 2_100_000_000_000_000)
            && exact_integer(&deposit.object_version)
            && deposit.object_version != "0"
            && exact_integer(&deposit.confirmations)
            && exact_integer(&deposit.required_confirmations)
            && DateTime::parse_from_rfc3339(&deposit.observed_at).is_ok();
    }
    if valid {
        Ok(())
    } else {
        Err(AxiomError::Protocol(
            "invalid mainnet deposit account".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_metadata_is_exact_and_creation_debug_redacts_the_token() {
        let raw = serde_json::json!({
            "id": "key-1", "name": "Coding tools", "scopes": ["inference"],
            "created_at": "2026-09-10T12:00:00Z", "last_used_at": null,
            "expires_at": null, "revoked_at": null, "usage_started_at": "2026-09-10T12:00:00Z",
            "usage": {"request_count": "1", "input_tokens": "9007199254740993",
                "cached_input_tokens": "100", "output_tokens": "12", "cost_microusd": "123456"}
        });
        let key: axiom_acp_extension::ApiKeyRecord = serde_json::from_value(raw.clone()).unwrap();
        validate_api_key_record(&key).unwrap();
        let projected = serde_json::to_value(&key).unwrap();
        assert_eq!(projected["usage"]["inputTokens"], "9007199254740993");
        assert_eq!(projected["usageStartedAt"], "2026-09-10T12:00:00Z");
        let result = axiom_acp_extension::ApiKeyCreatedResponse {
            key,
            token: "axm_keep_out_of_debug_output".into(),
        };
        assert!(!format!("{result:?}").contains("axm_keep_out"));
        for value in [
            "-1",
            "01",
            "1.1",
            "1e9",
            "",
            "340282366920938463463374607431768211456",
        ] {
            let mut bad = raw.clone();
            bad["usage"]["cost_microusd"] = value.into();
            assert!(validate_api_key_record(&serde_json::from_value(bad).unwrap()).is_err());
        }
        let mut bad = raw.clone();
        bad["usage"]["cached_input_tokens"] = "99999999999999999".into();
        assert!(validate_api_key_record(&serde_json::from_value(bad).unwrap()).is_err());
        let mut bad = raw;
        bad["scopes"] = serde_json::json!(["account"]);
        assert!(validate_api_key_record(&serde_json::from_value(bad).unwrap()).is_err());
    }

    #[test]
    fn usage_preserves_exact_costs_and_rejects_inconsistent_or_unbounded_data() {
        let valid = serde_json::json!({
            "period": "all_time", "total_cost_microusd": "9007199254740993",
            "models": [{ "provider": "test", "model_id": "model", "model_name": "Model",
                "cost_microusd": "9007199254740993" }]
        });
        let parsed = validated_usage_spend(
            serde_json::from_value(valid.clone()).unwrap(),
            axiom_acp_extension::UsagePeriod::AllTime,
        )
        .unwrap();
        assert_eq!(parsed.total_cost_microusd, "9007199254740993");
        for amount in ["9007199254740992", "-1", "01", "1e9", "1.5", ""] {
            let mut bad = valid.clone();
            bad["total_cost_microusd"] = amount.into();
            assert!(
                validated_usage_spend(
                    serde_json::from_value(bad).unwrap(),
                    axiom_acp_extension::UsagePeriod::AllTime
                )
                .is_err()
            );
        }
        let empty =
            serde_json::json!({ "period": "all_time", "total_cost_microusd": "0", "models": [] });
        assert!(
            validated_usage_spend(
                serde_json::from_value(empty).unwrap(),
                axiom_acp_extension::UsagePeriod::AllTime
            )
            .unwrap()
            .models
            .is_empty()
        );
        let mut bad = valid.clone();
        bad["models"][0]["model_id"] = serde_json::Value::Null;
        assert!(
            validated_usage_spend(
                serde_json::from_value(bad).unwrap(),
                axiom_acp_extension::UsagePeriod::AllTime
            )
            .is_err()
        );
        let mut bad = valid;
        bad["models"][0]["model_name"] = "bad\nname".into();
        assert!(
            validated_usage_spend(
                serde_json::from_value(bad).unwrap(),
                axiom_acp_extension::UsagePeriod::AllTime
            )
            .is_err()
        );
    }

    #[test]
    fn usage_requires_the_requested_period_including_legacy_server_responses() {
        use axiom_acp_extension::UsagePeriod;
        for requested in [UsagePeriod::Week, UsagePeriod::Month, UsagePeriod::AllTime] {
            for returned in ["week", "month", "all_time", "day"] {
                let raw = serde_json::json!({
                    "period": returned, "total_cost_microusd": "0", "models": []
                });
                assert_eq!(
                    validated_usage_spend(serde_json::from_value(raw).unwrap(), requested).is_ok(),
                    returned == requested.as_str()
                );
            }
        }
        let legacy: axiom_acp_extension::UsageSummaryRequest = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.period, UsagePeriod::AllTime);
        assert!(legacy.timezone.is_none());
    }

    #[test]
    fn persistent_deposits_preserve_exact_integers_and_reject_unsafe_checkout_data() {
        let address = format!("u1{}", "a".repeat(180));
        let payment: axiom_acp_extension::PaymentAccount =
            serde_json::from_value(serde_json::json!({
                "network": "mainnet", "asset": "ZEC", "conversion_status": "none",
                "state": "ready", "address": address, "payment_uri": format!("zcash:{address}"),
                "monitoring_status": "ready", "required_confirmations": "10",
                "confirmed_zatoshis": "9007199254740993", "confirming_zatoshis": "1",
                "review_required": false, "deposits": []
            }))
            .unwrap();
        assert!(validate_payment_account(&payment).is_ok());
        assert_eq!(payment.confirmed_zatoshis, "9007199254740993");
        for uri in [
            "https://example.com/pay",
            "zcash:another-address",
            "javascript:alert(1)",
        ] {
            let mut bad = payment.clone();
            bad.payment_uri = Some(uri.into());
            assert!(validate_payment_account(&bad).is_err());
        }
        for amount in ["01", "1.0", "-1", "18446744073709551615"] {
            let mut bad = payment.clone();
            bad.confirmed_zatoshis = amount.into();
            assert!(validate_payment_account(&bad).is_err());
        }
        let mut bad = payment.clone();
        bad.network = "testnet".into();
        assert!(validate_payment_account(&bad).is_err());
        let mut bad = payment.clone();
        bad.address = None;
        assert!(validate_payment_account(&bad).is_err());
        let mut bad = payment;
        bad.conversion_status = "converted_to_usdc".into();
        assert!(validate_payment_account(&bad).is_err());
    }

    #[test]
    fn deposit_valuation_is_exact_and_does_not_claim_currency_conversion() {
        let payment: axiom_acp_extension::PaymentAccount = serde_json::from_value(serde_json::json!({
            "network": "mainnet", "asset": "ZEC", "conversion_status": "none", "valuation_enabled": true,
            "state": "unprovisioned", "address": null, "payment_uri": null, "monitoring_status": "ready",
            "required_confirmations": "10", "confirmed_zatoshis": "10000000", "confirming_zatoshis": "0",
            "review_required": false, "deposits": [{
                "id": "20000000-0000-4000-8000-000000000001", "amount_zatoshis": "10000000",
                "state": "confirmed", "object_version": "2", "confirmations": "10", "required_confirmations": "10",
                "review_required": false, "observed_at": "2026-09-07T10:00:00Z", "valuation_status": "credited",
                "credit_microusd": "10000000", "price_microusd_per_zec": "100000000", "price_source": "coinbase",
                "priced_at": "2026-09-07T10:00:00Z"
            }]
        })).unwrap();
        assert!(validate_payment_account(&payment).is_ok());
        let mut wrong_amount = payment.clone();
        wrong_amount.deposits[0].credit_microusd = Some("10000001".into());
        assert!(validate_payment_account(&wrong_amount).is_err());
        let mut incomplete = payment.clone();
        incomplete.deposits[0].price_source = None;
        assert!(validate_payment_account(&incomplete).is_err());
        let mut fabricated = payment;
        fabricated.deposits[0].price_source = Some("invented".into());
        assert!(validate_payment_account(&fabricated).is_err());
    }

    #[test]
    fn backend_billing_shapes_are_validated_fail_closed() {
        let valid = BillingStatus {
            posted_microusd: 25_000_000,
            available_microusd: 25_000_000,
            ledger_sequence: 4,
            trial_microusd: 1_000_000,
            paid_microusd: 24_000_000,
            payment_review_required: false,
            currency: "microUSD".into(),
            payment_account: None,
            zec_usd_quote: None,
        };
        assert!(validate_billing_status(&valid).is_ok());
        let negative = BillingStatus {
            posted_microusd: -230_534,
            available_microusd: -230_534,
            trial_microusd: 0,
            paid_microusd: -230_534,
            ..valid.clone()
        };
        let decoded: BillingStatus = serde_json::from_str(
            r#"{
            "posted_microusd":-230534,"available_microusd":-230534,
            "trial_microusd":0,"paid_microusd":-230534,"ledger_sequence":4,
            "currency":"microUSD","payment_review_required":false,
            "payment_account":null,"zec_usd_quote":null
        }"#,
        )
        .unwrap();
        assert_eq!(decoded, negative);
        assert!(validate_billing_status(&negative).is_ok());
        assert!(
            validate_billing_status(&BillingStatus {
                available_microusd: 0,
                ..negative.clone()
            })
            .is_err()
        );
        assert!(
            validate_billing_status(&BillingStatus {
                trial_microusd: 1,
                paid_microusd: -230_535,
                ..negative
            })
            .is_err()
        );
        assert!(
            validate_billing_status(&BillingStatus {
                payment_review_required: true,
                available_microusd: 0,
                ..valid.clone()
            })
            .is_ok()
        );
        assert!(
            validate_billing_status(&BillingStatus {
                payment_review_required: true,
                ..valid.clone()
            })
            .is_err()
        );
        assert!(
            validate_billing_status(&BillingStatus {
                paid_microusd: 25_000_000,
                ..valid.clone()
            })
            .is_err()
        );
        let mut inconsistent = valid;
        inconsistent.available_microusd += 1;
        assert!(validate_billing_status(&inconsistent).is_err());

        let too_large = BillingStatus {
            posted_microusd: i64::try_from(MAX_ACP_SAFE_INTEGER).unwrap() + 1,
            available_microusd: i64::try_from(MAX_ACP_SAFE_INTEGER).unwrap() + 1,
            ..inconsistent
        };
        assert!(validate_billing_status(&too_large).is_err());
    }

    #[test]
    fn indicative_quotes_reject_expiry_future_prices_and_invalid_metadata() {
        let now = chrono::Utc::now();
        let quote = axiom_acp_extension::ZecUsdQuote {
            price_microusd_per_zec: "123456789".into(),
            source: "coinbase".into(),
            as_of: (now - chrono::Duration::seconds(5)).to_rfc3339(),
            expires_at: (now + chrono::Duration::seconds(55)).to_rfc3339(),
        };
        assert!(valid_current_quote(&quote, now));
        assert!(!valid_current_quote(
            &quote,
            now + chrono::Duration::seconds(55)
        ));
        assert!(!valid_current_quote(
            &quote,
            now - chrono::Duration::seconds(6)
        ));
        for price in ["0", "-1", "01", "1.5", "NaN", "1000000000001"] {
            let invalid = axiom_acp_extension::ZecUsdQuote {
                price_microusd_per_zec: price.into(),
                ..quote.clone()
            };
            assert!(!valid_current_quote(&invalid, now));
        }
        for source in ["unknown", "", "Coinbase"] {
            let invalid = axiom_acp_extension::ZecUsdQuote {
                source: source.into(),
                ..quote.clone()
            };
            assert!(!valid_current_quote(&invalid, now));
        }
        let extended = axiom_acp_extension::ZecUsdQuote {
            expires_at: (now + chrono::Duration::seconds(56)).to_rfc3339(),
            ..quote.clone()
        };
        assert!(!valid_current_quote(&extended, now));
        let malformed = axiom_acp_extension::ZecUsdQuote {
            as_of: "invalid".into(),
            ..quote
        };
        assert!(!valid_current_quote(&malformed, now));
    }
}
