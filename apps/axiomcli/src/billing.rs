//! Native-account billing and persistent mainnet deposit client.
//!
//! Billing is an account-control API, not an inference transport. It accepts
//! only short native-session access tokens and never falls back to the manual
//! automation key. Remote bodies and credentials are excluded from errors.

use std::time::Duration;

use chrono::DateTime;
use futures::StreamExt as _;
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{AxiomError, Result, auth::AuthManager};

pub const MAX_INVOICE_USD_CENTS: u64 = 10_000_000;
pub const MAX_CLIENT_REQUEST_ID_BYTES: usize = 128;
pub const MAX_INVOICE_ID_BYTES: usize = 128;
pub const MAX_PAYMENT_URL_BYTES: usize = 4_096;
const MAX_BILLING_RESPONSE_BYTES: usize = 64 * 1024;
// ACP's generated TypeScript contract represents integer fields as IEEE-754
// numbers. Reject projections that could lose precision at that boundary.
const MAX_ACP_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BillingStatus {
    pub posted_microusd: u64,
    pub held_microusd: u64,
    pub available_microusd: u64,
    pub ledger_sequence: u64,
    #[serde(default)]
    pub trial_microusd: Option<u64>,
    #[serde(default)]
    pub paid_microusd: Option<u64>,
    #[serde(default)]
    pub payment_review_required: bool,
    pub currency: String,
    pub invoices_enabled: bool,
    pub invoice_min_usd_cents: u64,
    pub invoice_max_usd_cents: u64,
    pub settlement_asset: Option<String>,
    #[serde(default)]
    pub payment_account: Option<axiom_acp_extension::PaymentAccount>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceState {
    Creating,
    Pending,
    Paid,
    Expired,
    Failed,
    Review,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Invoice {
    pub id: String,
    pub amount_usd_cents: u64,
    pub state: InvoiceState,
    pub expires_at: Option<String>,
    pub paid_at: Option<String>,
    pub pay_url: Option<String>,
}

#[derive(Serialize)]
struct InvoiceCreateRequest<'a> {
    amount_usd_cents: u64,
    client_request_id: &'a str,
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
        Ok(status)
    }

    pub async fn create_invoice(
        &self,
        amount_usd_cents: u64,
        client_request_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<Invoice> {
        validate_invoice_amount(amount_usd_cents)?;
        validate_client_request_id(client_request_id)?;
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        if cancellation.is_cancelled() {
            return Err(AxiomError::Cancelled);
        }
        // The client request ID makes a retry after cancellation or a lost
        // response idempotent at the backend.
        let response = send_with_cancellation(
            self.client
                .post(self.endpoint("/api/v1/billing/invoices"))
                .bearer_auth(lease.expose_for_authorization())
                .json(&InvoiceCreateRequest {
                    amount_usd_cents,
                    client_request_id,
                }),
            cancellation,
            "invoice creation",
        )
        .await?;
        ensure_success(response.status(), "invoice creation")?;
        let invoice = decode_bounded_json::<Invoice>(response, cancellation, "invoice").await?;
        validate_invoice(&invoice)?;
        if invoice.amount_usd_cents != amount_usd_cents {
            return Err(AxiomError::Protocol(
                "Axiom returned an invoice for a different amount".into(),
            ));
        }
        self.ensure_account_current(&lease)?;
        Ok(invoice)
    }

    pub async fn invoice_status(
        &self,
        invoice_id: &str,
        cancellation: &CancellationToken,
    ) -> Result<Invoice> {
        validate_invoice_id(invoice_id)?;
        let lease = self
            .auth
            .access_token_for_account_operation(cancellation)
            .await?;
        let mut endpoint = self.endpoint("/api/v1/billing/invoices/");
        endpoint
            .path_segments_mut()
            .expect("validated HTTP API origin supports path segments")
            .push(invoice_id);
        let response = send_with_cancellation(
            self.client
                .get(endpoint)
                .bearer_auth(lease.expose_for_authorization()),
            cancellation,
            "invoice status",
        )
        .await?;
        ensure_success(response.status(), "invoice status")?;
        let invoice = decode_bounded_json::<Invoice>(response, cancellation, "invoice").await?;
        validate_invoice(&invoice)?;
        if invoice.id != invoice_id {
            return Err(AxiomError::Protocol(
                "Axiom returned a different invoice identity".into(),
            ));
        }
        self.ensure_account_current(&lease)?;
        Ok(invoice)
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
    match (status.trial_microusd, status.paid_microusd) {
        (Some(trial), Some(paid))
            if trial <= MAX_ACP_SAFE_INTEGER
                && paid <= MAX_ACP_SAFE_INTEGER
                && trial.checked_add(paid) == Some(status.posted_microusd) => {}
        (None, None) => {} // Older backends do not report the split.
        _ => {
            return Err(AxiomError::Protocol(
                "Axiom returned inconsistent credit buckets".into(),
            ));
        }
    }
    if let Some(payment) = &status.payment_account {
        validate_payment_account(payment)?;
        if status.invoices_enabled || status.settlement_asset.is_some() {
            return Err(AxiomError::Protocol(
                "mainnet deposits cannot claim USDC settlement".into(),
            ));
        }
    }
    if status.posted_microusd > MAX_ACP_SAFE_INTEGER
        || status.held_microusd > MAX_ACP_SAFE_INTEGER
        || status.available_microusd > MAX_ACP_SAFE_INTEGER
        || status.ledger_sequence > MAX_ACP_SAFE_INTEGER
        || status.currency != "microUSD"
        || status.invoice_min_usd_cents == 0
        || status.invoice_max_usd_cents < status.invoice_min_usd_cents
        || status.invoice_max_usd_cents > MAX_INVOICE_USD_CENTS
        || status.held_microusd > status.posted_microusd
        || status.available_microusd
            != if status.payment_review_required {
                0
            } else {
                status.posted_microusd - status.held_microusd
            }
        || (status.invoices_enabled && status.settlement_asset.as_deref() != Some("USDC"))
        || (!status.invoices_enabled && status.settlement_asset.is_some())
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
        && value.parse::<u64>().is_ok_and(|n| n <= i64::MAX as u64)
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

fn validate_invoice_amount(amount_usd_cents: u64) -> Result<()> {
    if !(1..=MAX_INVOICE_USD_CENTS).contains(&amount_usd_cents) {
        return Err(AxiomError::InvalidTransition(
            "invoice amount is outside the supported USD-cent range".into(),
        ));
    }
    Ok(())
}

fn validate_client_request_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > MAX_CLIENT_REQUEST_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
    {
        return Err(AxiomError::InvalidTransition(
            "invoice client request ID is invalid".into(),
        ));
    }
    Ok(())
}

fn validate_invoice_id(value: &str) -> Result<()> {
    if value.len() != 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(AxiomError::InvalidTransition(
            "invoice ID is invalid".into(),
        ));
    }
    Ok(())
}

fn validate_invoice(invoice: &Invoice) -> Result<()> {
    validate_invoice_id(&invoice.id)?;
    validate_invoice_amount(invoice.amount_usd_cents)?;
    for timestamp in [&invoice.expires_at, &invoice.paid_at]
        .into_iter()
        .flatten()
    {
        DateTime::parse_from_rfc3339(timestamp).map_err(|_| {
            AxiomError::Protocol("Axiom returned an invalid invoice timestamp".into())
        })?;
    }
    if invoice.state == InvoiceState::Paid && invoice.paid_at.is_none() {
        return Err(AxiomError::Protocol(
            "a paid invoice omitted its payment timestamp".into(),
        ));
    }
    if let Some(pay_url) = &invoice.pay_url {
        validate_payment_url(pay_url)?;
        if !matches!(
            invoice.state,
            InvoiceState::Creating | InvoiceState::Pending
        ) {
            return Err(AxiomError::Protocol(
                "a terminal invoice unexpectedly retained a payment URL".into(),
            ));
        }
    }
    if invoice.state == InvoiceState::Pending && invoice.pay_url.is_none() {
        return Err(AxiomError::Protocol(
            "a pending invoice omitted its payment URL".into(),
        ));
    }
    Ok(())
}

pub fn validate_payment_url(value: &str) -> Result<Url> {
    if value.is_empty() || value.len() > MAX_PAYMENT_URL_BYTES {
        return Err(AxiomError::Protocol(
            "Axiom returned an invalid invoice payment URL".into(),
        ));
    }
    let url = Url::parse(value).map_err(|_| {
        AxiomError::Protocol("Axiom returned an invalid invoice payment URL".into())
    })?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(AxiomError::Protocol(
            "Axiom returned an untrusted invoice payment URL".into(),
        ));
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            held_microusd: 2_000_000,
            available_microusd: 23_000_000,
            ledger_sequence: 4,
            trial_microusd: Some(1_000_000),
            paid_microusd: Some(24_000_000),
            payment_review_required: false,
            currency: "microUSD".into(),
            invoices_enabled: true,
            invoice_min_usd_cents: 500,
            invoice_max_usd_cents: 100_000,
            settlement_asset: Some("USDC".into()),
            payment_account: None,
        };
        assert!(validate_billing_status(&valid).is_ok());
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
                paid_microusd: Some(25_000_000),
                ..valid.clone()
            })
            .is_err()
        );
        assert!(
            validate_billing_status(&BillingStatus {
                paid_microusd: None,
                ..valid.clone()
            })
            .is_err()
        );
        for unsupported in [Some("ZEC".into()), Some("USDT".into()), None] {
            let rejected = BillingStatus {
                settlement_asset: unsupported,
                ..valid.clone()
            };
            assert!(validate_billing_status(&rejected).is_err());
        }
        let mut inconsistent = valid;
        inconsistent.available_microusd += 1;
        assert!(validate_billing_status(&inconsistent).is_err());

        let too_large = BillingStatus {
            posted_microusd: MAX_ACP_SAFE_INTEGER + 1,
            available_microusd: MAX_ACP_SAFE_INTEGER + 1,
            ..inconsistent
        };
        assert!(validate_billing_status(&too_large).is_err());
    }

    #[test]
    fn invoice_ids_requests_and_payment_urls_are_bounded() {
        assert!(validate_invoice_id(&"a".repeat(32)).is_ok());
        assert!(validate_invoice_id(&"A".repeat(32)).is_err());
        assert!(validate_invoice_id("../other-account").is_err());
        assert!(validate_client_request_id("desktop:request-1").is_ok());
        assert!(validate_client_request_id("bad/request").is_err());
        assert!(validate_payment_url("https://shieldz.cash/pay/invoice?checkout=opaque").is_ok());
        for unsafe_url in [
            "http://shieldz.cash/pay/invoice",
            "https://user@shieldz.cash/pay/invoice",
            "https://shieldz.cash/pay/invoice#fragment",
            "file:///tmp/invoice",
        ] {
            assert!(validate_payment_url(unsafe_url).is_err());
        }
    }

    #[test]
    fn terminal_invoice_cannot_retain_checkout_capability() {
        let paid = Invoice {
            id: "a".repeat(32),
            amount_usd_cents: 2_500,
            state: InvoiceState::Paid,
            expires_at: Some("2026-09-04T01:00:00Z".into()),
            paid_at: Some("2026-09-04T00:30:00Z".into()),
            pay_url: Some("https://shieldz.cash/pay/invoice".into()),
        };
        assert!(validate_invoice(&paid).is_err());
    }
}
