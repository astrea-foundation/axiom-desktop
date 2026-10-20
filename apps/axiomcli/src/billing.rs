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
// Deposit provisioning/receipt display is optional once the ledger is loaded.
// Bound the whole operation, including response decoding, independently of the
// much longer inference timeout. A shorter configured HTTP deadline still wins.
const OPTIONAL_DEPOSIT_TIMEOUT: Duration = Duration::from_secs(5);
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
    #[serde(default)]
    pub trial_capacity: Option<Box<axiom_acp_extension::TrialCapacity>>,
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
        if status.payment_account.as_ref().is_some_and(|account| {
            account.state != "ready"
                || account
                    .deposits
                    .iter()
                    .any(|deposit| deposit.state == "confirming")
        }) {
            // Idle ready addresses already arrive in the read-only projection.
            // Pending receipts still need this endpoint for per-block display
            // progress: backend events describe transitions, not every block.
            // Lost/timed-out provisioning reuses the backend's durable keys.
            let payment = match tokio::time::timeout(
                OPTIONAL_DEPOSIT_TIMEOUT,
                self.provision_deposit_address(&lease, cancellation),
            )
            .await
            {
                Ok(result) => result?,
                Err(_) => None,
            };
            if let Some(payment) = payment {
                status.payment_account = Some(payment);
                validate_billing_status(&status)?;
            }
        }
        // Expired/invalid indicative data must not hide an otherwise valid balance.
        status.zec_usd_quote = status
            .zec_usd_quote
            .filter(|quote| valid_current_quote(quote, chrono::Utc::now()));
        // Optional deposit work never extends the lifetime or account scope of
        // the financial projection obtained above. A cancellation or account
        // change must win even when provisioning was temporarily unavailable.
        if cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        self.ensure_account_current(&lease)?;
        Ok(status)
    }

    /// A deposit-service outage must not discard an authenticated, validated
    /// ledger balance. On a transient failure retain the original validated
    /// deposit state so the caller can safely retry; never invent an address,
    /// credit, or successful provisioning state. Invalid responses and access
    /// rejections still fail closed rather than being treated as an outage.
    async fn provision_deposit_address(
        &self,
        lease: &crate::auth::AccountAccessTokenLease,
        cancellation: &CancellationToken,
    ) -> Result<Option<axiom_acp_extension::PaymentAccount>> {
        let response = match send_with_cancellation(
            self.client
                .post(self.endpoint("/api/v1/billing/deposit-address"))
                .bearer_auth(lease.expose_for_authorization())
                .json(&serde_json::json!({})),
            cancellation,
            "deposit address",
        )
        .await
        {
            // This helper's Provider error comes only from the transport.
            // Remote bodies and credentials are not logged or surfaced here.
            Err(AxiomError::Provider(_)) => return Ok(None),
            response => response?,
        };
        if matches!(response.status().as_u16(), 408 | 425 | 429 | 500..=599) {
            return Ok(None);
        }
        ensure_success(response.status(), "deposit address")?;
        let payment = decode_bounded_json::<axiom_acp_extension::PaymentAccount>(
            response,
            cancellation,
            "deposit-address",
        )
        .await?;
        validate_payment_account(&payment)?;
        Ok(Some(payment))
    }

    async fn crypto_request<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: Option<serde_json::Value>,
        cancellation: &CancellationToken,
    ) -> Result<Option<T>> {
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let builder = if let Some(body) = body {
            self.client.post(self.endpoint(path)).json(&body)
        } else {
            self.client.get(self.endpoint(path))
        };
        let response = send_with_cancellation(
            builder.bearer_auth(lease.expose_for_authorization()),
            cancellation,
            "crypto payment",
        )
        .await?;
        self.ensure_account_current(&lease)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if response.status() == StatusCode::UNPROCESSABLE_ENTITY {
            return Err(AxiomError::Provider(
                "Increase the amount or choose another crypto.".into(),
            ));
        }
        if response.status() == StatusCode::CONFLICT {
            let value =
                decode_bounded_json::<serde_json::Value>(response, cancellation, "crypto-payment")
                    .await?;
            self.ensure_account_current(&lease)?;
            let message = if value
                .pointer("/detail/code")
                .and_then(serde_json::Value::as_str)
                == Some("invoice_capacity")
            {
                "You have 5 active invoices. Cancel an invoice to create another."
            } else {
                "Invoice unavailable. Refresh its status before trying again."
            };
            return Err(AxiomError::Provider(message.into()));
        }
        ensure_success(response.status(), "crypto payment")?;
        let result = decode_bounded_json(response, cancellation, "crypto-payment").await?;
        self.ensure_account_current(&lease)?;
        Ok(Some(result))
    }

    pub async fn crypto_options(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<axiom_acp_extension::CryptoOptions> {
        let options = self
            .crypto_request("/api/v1/billing/crypto/options", None, cancellation)
            .await?
            .unwrap_or(axiom_acp_extension::CryptoOptions {
                enabled: false,
                zcash_discount_bps: 0,
                min_amount_microusd: 5_000_000,
                max_amount_microusd: 1_000_000_000,
                currencies: vec![],
            });
        validate_crypto_options(&options)?;
        Ok(options)
    }

    pub async fn create_crypto_payment(
        &self,
        request: &axiom_acp_extension::CreateCryptoPaymentRequest,
        cancellation: &CancellationToken,
    ) -> Result<axiom_acp_extension::CryptoPayment> {
        if uuid::Uuid::parse_str(&request.id).is_err()
            || !valid_crypto_amount(request.amount_microusd)
            || !valid_crypto_currency(&request.pay_currency)
            || request.pay_currency == "zec"
        {
            return Err(AxiomError::Config("Invalid payment request.".into()));
        }
        let payment = self.crypto_request("/api/v1/billing/crypto/payments", Some(serde_json::json!({
            "id": request.id, "amount_microusd": request.amount_microusd, "pay_currency": request.pay_currency
        })), cancellation).await?.ok_or_else(|| AxiomError::Provider("Crypto payments are unavailable.".into()))?;
        validate_crypto_payment(&payment)?;
        if payment.id != request.id
            || payment.amount_microusd != request.amount_microusd
            || payment.pay_currency != request.pay_currency
        {
            return Err(AxiomError::Protocol(
                "Payment request does not match its response.".into(),
            ));
        }
        Ok(payment)
    }

    pub async fn crypto_invoice_action(
        &self,
        request: &axiom_acp_extension::CryptoInvoiceActionRequest,
        cancellation: &CancellationToken,
    ) -> Result<axiom_acp_extension::CryptoPayment> {
        use axiom_acp_extension::CryptoInvoiceAction;
        let invoice = uuid::Uuid::parse_str(&request.invoice_id)
            .map_err(|_| AxiomError::Protocol("Invalid invoice ID.".into()))?;
        let (action, body) = match request.action {
            CryptoInvoiceAction::Refresh => {
                let id = request
                    .id
                    .as_deref()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .ok_or_else(|| AxiomError::Protocol("Invalid quote ID.".into()))?;
                let previous = request
                    .previous_quote_id
                    .as_deref()
                    .and_then(|s| uuid::Uuid::parse_str(s).ok())
                    .ok_or_else(|| AxiomError::Protocol("Invalid previous quote ID.".into()))?;
                (
                    "refresh",
                    serde_json::json!({"id":id.to_string(),"previous_quote_id":previous.to_string()}),
                )
            }
            action => {
                if request.id.is_some() || request.previous_quote_id.is_some() {
                    return Err(AxiomError::Protocol("Unexpected invoice fields.".into()));
                }
                (
                    if action == CryptoInvoiceAction::Cancel {
                        "cancel"
                    } else {
                        "check"
                    },
                    serde_json::json!({}),
                )
            }
        };
        let payment: axiom_acp_extension::CryptoPayment = self
            .crypto_request(
                &format!("/api/v1/billing/crypto/payments/{invoice}/{action}"),
                Some(body),
                cancellation,
            )
            .await?
            .ok_or_else(|| AxiomError::Provider("Invoices are unavailable.".into()))?;
        validate_crypto_payment(&payment)?;
        if payment.id != invoice.to_string() {
            return Err(AxiomError::Protocol("Invoice identity changed.".into()));
        }
        Ok(payment)
    }

    pub async fn crypto_payments(
        &self,
        cancellation: &CancellationToken,
    ) -> Result<Vec<axiom_acp_extension::CryptoPayment>> {
        let payments: Vec<_> = self
            .crypto_request("/api/v1/billing/crypto/payments", None, cancellation)
            .await?
            .unwrap_or_default();
        if payments.len() > 20 {
            return Err(AxiomError::Protocol("Invalid payment history.".into()));
        }
        for payment in &payments {
            validate_crypto_payment(payment)?;
        }
        Ok(payments)
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

fn valid_crypto_currency(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn valid_crypto_amount(value: u64) -> bool {
    (5_000_000..=1_000_000_000).contains(&value) && value.is_multiple_of(10_000)
}

fn validate_crypto_options(options: &axiom_acp_extension::CryptoOptions) -> Result<()> {
    let clean = |s: &str| !s.is_empty() && s.len() <= 320 && !s.chars().any(char::is_control);
    let mut codes = std::collections::HashSet::new();
    if !matches!(options.zcash_discount_bps, 0 | 500)
        || options.min_amount_microusd != 5_000_000
        || options.max_amount_microusd != 1_000_000_000
        || options.currencies.len() > 128
        || (!options.enabled && !options.currencies.is_empty())
        || options.currencies.iter().any(|c| {
            !valid_crypto_currency(&c.code)
                || c.code == "zec"
                || !clean(&c.name)
                || !clean(&c.network)
                || !codes.insert(&c.code)
        })
    {
        return Err(AxiomError::Protocol(
            "Invalid crypto payment options.".into(),
        ));
    }
    Ok(())
}

fn validate_crypto_payment(payment: &axiom_acp_extension::CryptoPayment) -> Result<()> {
    let invoice = match (
        &payment.quote_id,
        &payment.invoice_expires_at,
        payment.active,
    ) {
        (None, None, None) => true,
        (Some(quote), Some(end), Some(active)) => {
            match (
                DateTime::parse_from_rfc3339(&payment.created_at),
                DateTime::parse_from_rfc3339(end),
            ) {
                (Ok(created), Ok(end)) => {
                    uuid::Uuid::parse_str(quote).is_ok()
                        && end - created == chrono::Duration::hours(6)
                        && payment.expires_at.as_deref().is_none_or(|rate| {
                            DateTime::parse_from_rfc3339(rate).is_ok_and(|rate| rate <= end)
                        })
                        && (!active
                            || (payment.status != "cancelled" && payment.credited_microusd == 0))
                }
                _ => false,
            }
        }
        _ => false,
    };
    let amount = payment.pay_amount.as_deref().is_none_or(|s| {
        s.len() <= 80
            && s.bytes().all(|c| c.is_ascii_digit() || c == b'.')
            && s.split('.').count() <= 2
            && s.bytes().any(|c| (b'1'..=b'9').contains(&c))
            && !s.starts_with('.')
            && !s.ends_with('.')
    });
    let address = payment.pay_address.as_deref().is_none_or(|s| {
        !s.is_empty()
            && s.len() <= 512
            && s.is_ascii()
            && !s.bytes().any(|c| c <= 32 || c == 127 || c == b':')
    });
    let memo = payment
        .payin_extra_id
        .as_deref()
        .is_none_or(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control));
    let expiry = payment
        .expires_at
        .as_deref()
        .is_none_or(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok());
    if uuid::Uuid::parse_str(&payment.id).is_err()
        || !valid_crypto_amount(payment.amount_microusd)
        || payment.credited_microusd > MAX_ACP_SAFE_INTEGER
        || !payment
            .credited_microusd
            .is_multiple_of(payment.amount_microusd)
        || !valid_crypto_currency(&payment.pay_currency)
        || payment.pay_currency == "zec"
        || !matches!(
            payment.status.as_str(),
            "creating"
                | "waiting"
                | "confirming"
                | "confirmed"
                | "sending"
                | "partially_paid"
                | "finished"
                | "failed"
                | "expired"
                | "refunded"
                | "review"
                | "cancelled"
        )
        || !amount
        || !address
        || !memo
        || !expiry
        || !invoice
        || chrono::DateTime::parse_from_rfc3339(&payment.created_at).is_err()
        || (payment.pay_address.is_some() != payment.pay_amount.is_some())
        || (payment.pay_address.is_some() && payment.expires_at.is_none())
    {
        return Err(AxiomError::Protocol(
            "Invalid crypto payment details.".into(),
        ));
    }
    Ok(())
}

fn validate_billing_status(status: &BillingStatus) -> Result<()> {
    if status.trial_capacity.as_ref().is_some_and(|capacity| {
        capacity.capacity_microusd == 0
            || capacity.capacity_microusd > 1_000_000_000
            || !exact_integer(&capacity.spent_microusd)
            || capacity.available
                != capacity
                    .spent_microusd
                    .parse::<u64>()
                    .is_ok_and(|spent| spent < capacity.capacity_microusd)
            || (capacity.available != capacity.retry_at.is_none())
            || capacity
                .retry_at
                .as_deref()
                .is_some_and(|at| DateTime::parse_from_rfc3339(at).is_err())
    }) {
        return Err(AxiomError::Protocol("Invalid trial capacity status".into()));
    }
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

    /// Exercise the real native-session and HTTP path with an isolated
    /// in-memory credential store. No real account, Keychain entry, payment
    /// service, or provider is contacted by these fixtures.
    #[cfg(debug_assertions)]
    mod status_http {
        use super::*;
        use std::sync::{
            Arc, RwLock,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        };

        use axum::{
            Json, Router,
            extract::State,
            http::{HeaderMap, StatusCode as HttpStatus},
            response::{IntoResponse as _, Response as HttpResponse},
            routing::{delete, get, post},
        };
        use serde_json::{Value, json};
        use tokio::sync::Notify;

        struct ServerState {
            billing: RwLock<Value>,
            payment: RwLock<Value>,
            provision_status: AtomicUsize,
            provision_calls: AtomicUsize,
            block_provision: AtomicBool,
            provision_started: Notify,
            provision_release: Notify,
        }

        struct Fixture {
            client: BillingClient,
            state: Arc<ServerState>,
            server: tokio::task::JoinHandle<()>,
            // Keep the synthetic credential-lock paths alive until all HTTP
            // operations end. The actual user's platform paths are never used.
            _directory: tempfile::TempDir,
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                self.server.abort();
            }
        }

        fn payment(ready: bool) -> Value {
            let address = format!("u1{}", "a".repeat(40));
            json!({
                "network": "mainnet", "asset": "ZEC", "conversion_status": "none",
                "valuation_enabled": true,
                "state": if ready { "ready" } else { "unprovisioned" },
                "address": if ready { Some(address.clone()) } else { None },
                "payment_uri": if ready { Some(format!("zcash:{address}")) } else { None },
                "monitoring_status": if ready { "ready" } else { "unprovisioned" },
                "required_confirmations": null, "confirmed_zatoshis": "0",
                "confirming_zatoshis": "0", "review_required": false, "deposits": []
            })
        }

        fn check_native_bearer(headers: &HeaderMap) {
            assert_eq!(
                headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok()),
                Some("Bearer access-token-with-enough-entropy-123456789")
            );
        }

        async fn create_login(Json(request): Json<Value>) -> Json<Value> {
            // This URL is only validated as login metadata; the test never
            // launches a browser or sends a request to the auth website.
            let origin = std::env::var("AXIOM_AUTH_URL")
                .unwrap_or_else(|_| "https://auth.axiom.stream".into());
            Json(json!({
                "id": "billing-test-authorization",
                "device_code": "device-code-with-enough-entropy-123456789",
                "user_code": "AXIOM-TEST",
                "state": "billing-state-with-enough-entropy-123456789",
                "authorization_url": format!("{}/native/authorize?authorization_id=billing-test-authorization&state=billing-state-with-enough-entropy-123456789", origin.trim_end_matches('/')),
                "callback_uri": request["callback_uri"],
                "expires_at": (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339(),
                "interval_seconds": 1
            }))
        }

        async fn exchange_login() -> Json<Value> {
            let expiry = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
            Json(json!({
                "status": "complete", "token_type": "bearer",
                "access_token": "access-token-with-enough-entropy-123456789",
                "access_expires_at": expiry,
                "refresh_token": "refresh-token-with-enough-entropy-123456789",
                "refresh_expires_at": expiry,
                "account": {"id": "billing-test-account", "display_name": null,
                    "avatar_url": null, "verified_email": null, "linked_methods": []},
                "session": {"id": "billing-test-session", "expires_at": expiry}
            }))
        }

        async fn get_status(
            State(state): State<Arc<ServerState>>,
            headers: HeaderMap,
        ) -> Json<Value> {
            check_native_bearer(&headers);
            Json(state.billing.read().expect("billing fixture").clone())
        }

        async fn provision(
            State(state): State<Arc<ServerState>>,
            headers: HeaderMap,
            Json(request): Json<Value>,
        ) -> HttpResponse {
            check_native_bearer(&headers);
            assert_eq!(request, json!({}));
            state.provision_calls.fetch_add(1, Ordering::AcqRel);
            if state.block_provision.load(Ordering::Acquire) {
                state.provision_started.notify_one();
                state.provision_release.notified().await;
            }
            let status = HttpStatus::from_u16(
                u16::try_from(state.provision_status.load(Ordering::Acquire))
                    .expect("fixture HTTP status range"),
            )
            .expect("fixture HTTP status");
            if !status.is_success() {
                return (status, "remote error body must not be reflected").into_response();
            }
            (
                status,
                Json(state.payment.read().expect("payment fixture").clone()),
            )
                .into_response()
        }

        async fn fixture(ready: bool) -> Fixture {
            let state = Arc::new(ServerState {
                billing: RwLock::new(json!({
                    "posted_microusd": 3_500_000, "available_microusd": 3_500_000,
                    "ledger_sequence": 2, "trial_microusd": 1_000_000,
                    "paid_microusd": 2_500_000, "payment_review_required": false,
                    "currency": "microUSD", "payment_account": payment(ready),
                    "zec_usd_quote": null, "trial_capacity": null
                })),
                payment: RwLock::new(payment(true)),
                provision_status: AtomicUsize::new(200),
                provision_calls: AtomicUsize::new(0),
                block_provision: AtomicBool::new(false),
                provision_started: Notify::new(),
                provision_release: Notify::new(),
            });
            let router = Router::new()
                .route("/api/v1/auth/native/authorizations", post(create_login))
                .route("/api/v1/auth/native/exchange", post(exchange_login))
                .route(
                    "/api/v1/auth/session",
                    delete(|| async { HttpStatus::NO_CONTENT }),
                )
                .route("/api/v1/billing/status", get(get_status))
                .route("/api/v1/billing/deposit-address", post(provision))
                .with_state(Arc::clone(&state));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("listener");
            let origin = format!("http://{}", listener.local_addr().expect("address"));
            let server = tokio::spawn(async move {
                axum::serve(listener, router)
                    .await
                    .expect("billing fixture server");
            });
            let directory = tempfile::tempdir().expect("isolated native account directory");
            let paths = crate::paths::AxiomPaths::from_roots(
                directory.path().join("config"),
                directory.path().join("data"),
            );
            let auth = AuthManager::new_ephemeral_test(&origin, Duration::from_secs(2), &paths)
                .expect("ephemeral auth");
            auth.start_native_login(None)
                .await
                .expect("native login")
                .complete(CancellationToken::new())
                .await
                .expect("native session");
            let client =
                BillingClient::new(&origin, auth, Duration::from_secs(2)).expect("billing client");
            Fixture {
                client,
                state,
                server,
                _directory: directory,
            }
        }

        #[tokio::test]
        async fn ready_balances_refresh_without_deposit_provisioning() {
            let fixture = fixture(true).await;
            // A failing deposit service must be irrelevant once its validated
            // ready address is already included in the read-only projection.
            fixture.state.provision_status.store(503, Ordering::Release);
            for _ in 0..2 {
                let status = fixture
                    .client
                    .status(&CancellationToken::new())
                    .await
                    .expect("balance");
                assert_eq!(status.available_microusd, 3_500_000);
                assert_eq!(
                    status.payment_account.expect("payment account").state,
                    "ready"
                );
            }
            assert_eq!(fixture.state.provision_calls.load(Ordering::Acquire), 0);
        }

        #[tokio::test]
        async fn transient_deposit_failures_preserve_validated_balance_and_pending_state() {
            let fixture = fixture(false).await;
            for status_code in [408, 425, 429, 500, 503] {
                fixture
                    .state
                    .provision_status
                    .store(status_code, Ordering::Release);
                let status = fixture
                    .client
                    .status(&CancellationToken::new())
                    .await
                    .expect("balance survives deposit outage");
                assert_eq!(status.available_microusd, 3_500_000);
                let payment = status.payment_account.expect("pending account");
                assert_eq!(payment.state, "unprovisioned");
                assert_eq!(payment.address, None);
                assert_eq!(payment.payment_uri, None);
            }
            assert_eq!(fixture.state.provision_calls.load(Ordering::Acquire), 5);
        }

        #[tokio::test]
        async fn ready_accounts_still_refresh_confirming_receipts_without_hiding_credit() {
            let fixture = fixture(true).await;
            let mut pending = payment(true);
            pending["confirming_zatoshis"] = json!("1000");
            pending["deposits"] = json!([{
                "id": "20000000-0000-4000-8000-000000000001", "amount_zatoshis": "1000",
                "state": "confirming", "object_version": "1", "confirmations": "1",
                "required_confirmations": "10", "review_required": false,
                "observed_at": "2026-10-20T00:00:00Z", "valuation_status": "pending"
            }]);
            fixture.state.billing.write().expect("billing fixture")["payment_account"] =
                pending.clone();
            let mut current = pending;
            current["deposits"][0]["confirmations"] = json!("6");
            *fixture.state.payment.write().expect("payment fixture") = current;
            let status = fixture
                .client
                .status(&CancellationToken::new())
                .await
                .expect("receipt refresh");
            assert_eq!(status.available_microusd, 3_500_000);
            assert_eq!(
                status.payment_account.expect("ready account").deposits[0].confirmations,
                "6"
            );
            fixture.state.provision_status.store(503, Ordering::Release);
            let status = fixture
                .client
                .status(&CancellationToken::new())
                .await
                .expect("balance survives receipt outage");
            let payment = status.payment_account.expect("validated account retained");
            assert_eq!(status.available_microusd, 3_500_000);
            assert_eq!(payment.state, "ready");
            assert!(payment.address.is_some());
            assert_eq!(payment.deposits[0].confirmations, "1");
            assert_eq!(fixture.state.provision_calls.load(Ordering::Acquire), 2);
        }

        #[tokio::test]
        async fn optional_deposit_work_does_not_wait_for_the_long_request_timeout() {
            let fixture = fixture(false).await;
            fixture.state.block_provision.store(true, Ordering::Release);
            let client = BillingClient::new(
                fixture.client.api_base_url.as_str(),
                fixture.client.auth.clone(),
                Duration::from_secs(30),
            )
            .expect("long request timeout");
            let cancellation = CancellationToken::new();
            let status = tokio::time::timeout(
                OPTIONAL_DEPOSIT_TIMEOUT + Duration::from_secs(2),
                client.status(&cancellation),
            )
            .await
            .expect("optional deposit deadline")
            .expect("validated balance");
            assert_eq!(status.available_microusd, 3_500_000);
            let payment = status.payment_account.expect("pending account");
            assert_eq!(payment.state, "unprovisioned");
            assert_eq!(payment.address, None);
            fixture.state.provision_release.notify_one();
        }

        #[tokio::test]
        async fn deposit_transport_timeout_preserves_validated_balance() {
            let fixture = fixture(false).await;
            fixture.state.block_provision.store(true, Ordering::Release);
            let client = BillingClient::new(
                fixture.client.api_base_url.as_str(),
                fixture.client.auth.clone(),
                Duration::from_secs(1),
            )
            .expect("short bounded client");
            let status = client
                .status(&CancellationToken::new())
                .await
                .expect("balance survives timeout");
            assert_eq!(status.available_microusd, 3_500_000);
            assert_eq!(
                status.payment_account.expect("pending account").state,
                "unprovisioned"
            );
            fixture.state.provision_release.notify_one();
        }

        #[tokio::test]
        async fn successful_provisioning_retains_ledger_credit_and_validates_the_address() {
            let fixture = fixture(false).await;
            let status = fixture
                .client
                .status(&CancellationToken::new())
                .await
                .expect("provisioned status");
            assert_eq!(status.available_microusd, 3_500_000);
            assert_eq!(status.paid_microusd, 2_500_000);
            assert_eq!(
                status.payment_account.expect("ready account").state,
                "ready"
            );
            assert_eq!(fixture.state.provision_calls.load(Ordering::Acquire), 1);
        }

        #[tokio::test]
        async fn malformed_deposit_or_ledger_data_and_access_rejections_fail_closed() {
            let fixture = fixture(false).await;
            for status in [400, 401, 403] {
                fixture
                    .state
                    .provision_status
                    .store(status, Ordering::Release);
                let error = fixture
                    .client
                    .status(&CancellationToken::new())
                    .await
                    .expect_err("access rejection");
                assert!(!error.to_string().contains("remote error body"));
            }
            fixture.state.provision_status.store(200, Ordering::Release);
            for invalid in [json!({"unexpected": true}), {
                let mut invalid = payment(true);
                invalid["payment_uri"] = json!("https://untrusted.example/");
                invalid
            }] {
                *fixture.state.payment.write().expect("payment fixture") = invalid;
                assert!(matches!(
                    fixture.client.status(&CancellationToken::new()).await,
                    Err(AxiomError::Protocol(_))
                ));
            }
            let calls = fixture.state.provision_calls.load(Ordering::Acquire);
            fixture.state.billing.write().expect("billing fixture")["available_microusd"] =
                json!(9_000_000);
            assert!(matches!(
                fixture.client.status(&CancellationToken::new()).await,
                Err(AxiomError::Protocol(_))
            ));
            assert_eq!(fixture.state.provision_calls.load(Ordering::Acquire), calls);
        }

        #[tokio::test]
        async fn pending_provisioning_does_not_publish_after_cancellation() {
            let fixture = fixture(false).await;
            fixture.state.provision_status.store(503, Ordering::Release);
            fixture.state.block_provision.store(true, Ordering::Release);
            let cancellation = CancellationToken::new();
            let status = fixture.client.status(&cancellation);
            tokio::pin!(status);
            tokio::select! {
                biased;
                _ = &mut status => panic!("provisioning must be pending"),
                () = fixture.state.provision_started.notified() => {}
            }
            cancellation.cancel();
            assert!(matches!(status.await, Err(AxiomError::Cancelled)));
            fixture.state.provision_release.notify_one();
        }

        #[tokio::test]
        async fn pending_provisioning_does_not_publish_after_logout() {
            let fixture = fixture(false).await;
            fixture.state.provision_status.store(503, Ordering::Release);
            fixture.state.block_provision.store(true, Ordering::Release);
            let next_cancellation = CancellationToken::new();
            let next = fixture.client.status(&next_cancellation);
            tokio::pin!(next);
            tokio::select! {
                biased;
                _ = &mut next => panic!("provisioning must be pending"),
                () = fixture.state.provision_started.notified() => {}
            }
            fixture.client.auth.logout_async().await.expect("logout");
            fixture.state.provision_release.notify_one();
            let error = next
                .await
                .expect_err("stale financial projection must be discarded");
            assert!(error.to_string().contains("account changed"));
        }
    }

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
            trial_capacity: None,
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
    #[test]
    fn crypto_payment_contract_rejects_unsafe_amounts_and_addresses() {
        assert!(valid_crypto_currency("s"));
        assert!(valid_crypto_currency("x"));
        assert!(!valid_crypto_currency(""));
        assert!(!valid_crypto_currency("X"));
        let now = chrono::Utc::now().to_rfc3339();
        let mut payment = axiom_acp_extension::CryptoPayment {
            quote_id: None,
            invoice_expires_at: None,
            active: None,
            id: uuid::Uuid::new_v4().to_string(),
            status: "waiting".into(),
            amount_microusd: 25_000_000,
            credited_microusd: 0,
            pay_currency: "btc".into(),
            pay_amount: Some("0.00050001".into()),
            pay_address: Some("bc1qtestaddress".into()),
            payin_extra_id: Some("12345".into()),
            expires_at: Some(now.clone()),
            created_at: now,
            review_required: false,
        };
        assert!(validate_crypto_payment(&payment).is_ok());
        for amount in ["NaN", "1e-6", "0", ".5", "1.", "1.2.3", "-1"] {
            payment.pay_amount = Some(amount.into());
            assert!(validate_crypto_payment(&payment).is_err());
        }
        payment.pay_amount = Some("0.00050001".into());
        payment.pay_address = Some("https://untrusted.example".into());
        assert!(validate_crypto_payment(&payment).is_err());
        payment.pay_address = Some("bc1qtestaddress".into());
        payment.credited_microusd = 24_000_000;
        assert!(validate_crypto_payment(&payment).is_err());
        payment.credited_microusd = 0;
        payment.quote_id = Some(uuid::Uuid::new_v4().to_string());
        payment.active = Some(true);
        let end =
            DateTime::parse_from_rfc3339(&payment.created_at).unwrap() + chrono::Duration::hours(6);
        payment.invoice_expires_at = Some(end.to_rfc3339());
        assert!(validate_crypto_payment(&payment).is_ok());
        payment.invoice_expires_at = Some((end + chrono::Duration::seconds(1)).to_rfc3339());
        assert!(validate_crypto_payment(&payment).is_err());
        payment.invoice_expires_at = Some(end.to_rfc3339());
        payment.quote_id = None;
        assert!(validate_crypto_payment(&payment).is_err());
        let mut options = axiom_acp_extension::CryptoOptions {
            enabled: true,
            zcash_discount_bps: 500,
            min_amount_microusd: 5_000_000,
            max_amount_microusd: 1_000_000_000,
            currencies: vec![axiom_acp_extension::CryptoCurrency {
                code: "btc".into(),
                name: "Bitcoin".into(),
                network: "Bitcoin".into(),
            }],
        };
        assert!(validate_crypto_options(&options).is_ok());
        options.currencies.push(options.currencies[0].clone());
        assert!(validate_crypto_options(&options).is_err());
    }
}
