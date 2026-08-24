use std::collections::BTreeSet;

use axiom_inference::{ModelInfo, ReasoningEffort};
use reqwest::StatusCode;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    ApiCredential, ProviderRegistry, Result, SecureClientConfig, SecureClientError,
    http::{bounded_body, pinned_client},
};

const MAX_MODELS: usize = 512;
const MAX_MODEL_TOKENS: u32 = 16 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogModel {
    id: String,
    label: String,
    short_label: String,
    model: String,
    base_url: String,
    provider: String,
    provider_label: String,
    e2ee_protocol: String,
    e2ee_encryption_version: u16,
    attestation_protocol: String,
    context_window_tokens: u32,
    max_output_tokens: u32,
    #[serde(default)]
    supported_reasoning_efforts: Vec<ReasoningEffort>,
}

pub(crate) async fn fetch_models(
    config: &SecureClientConfig,
    credential: &ApiCredential,
    providers: &ProviderRegistry,
    cancellation: &CancellationToken,
) -> Result<Vec<ModelInfo>> {
    if cancellation.is_cancelled() {
        return Err(SecureClientError::cancelled());
    }
    let endpoint = relay_endpoint(&config.relay_base_url, "/api/v1/relay/models")?;
    let client = pinned_client(
        &endpoint,
        &config.endpoint_policy,
        config.request_timeout,
        false,
    )
    .await?;
    let response = tokio::select! {
        () = cancellation.cancelled() => return Err(SecureClientError::cancelled()),
        response = client
            .get(endpoint)
            .header("accept", "application/json")
            .bearer_auth(credential.expose())
            .send() => response.map_err(|_| SecureClientError::new(
                axiom_inference::ProviderFailureKind::Transient,
                "model catalog request failed",
            ))?,
    };
    if response.status() != StatusCode::OK {
        return Err(error_for_status(response.status()));
    }
    let body = bounded_body(response, config.limits.model_catalog_bytes).await?;
    let wire: Vec<CatalogModel> = serde_json::from_slice(&body)
        .map_err(|_| SecureClientError::catalog("model catalog schema is invalid"))?;
    validate_models(wire, config, providers).await
}

async fn validate_models(
    wire: Vec<CatalogModel>,
    config: &SecureClientConfig,
    providers: &ProviderRegistry,
) -> Result<Vec<ModelInfo>> {
    if wire.is_empty() || wire.len() > MAX_MODELS {
        return Err(SecureClientError::catalog(
            "model catalog has an invalid number of entries",
        ));
    }
    let mut ids = BTreeSet::new();
    let mut output = Vec::with_capacity(wire.len());
    for item in wire {
        validate_identifier(&item.id, 80, "model ID is invalid")?;
        if !ids.insert(item.id.clone()) {
            return Err(SecureClientError::catalog(
                "model catalog contains duplicate IDs",
            ));
        }
        validate_text(&item.label, 200, "model label is invalid")?;
        validate_text(&item.short_label, 100, "model short label is invalid")?;
        validate_text(&item.model, 256, "upstream model name is invalid")?;
        validate_identifier(&item.provider, 64, "provider ID is invalid")?;
        validate_text(&item.provider_label, 100, "provider label is invalid")?;
        validate_identifier(&item.e2ee_protocol, 64, "E2EE protocol ID is invalid")?;
        validate_identifier(
            &item.attestation_protocol,
            64,
            "attestation protocol ID is invalid",
        )?;
        if item.context_window_tokens == 0
            || item.max_output_tokens == 0
            || item.context_window_tokens > MAX_MODEL_TOKENS
            || item.max_output_tokens > item.context_window_tokens
        {
            return Err(SecureClientError::catalog("model token limits are invalid"));
        }
        let mut efforts = BTreeSet::new();
        for effort in &item.supported_reasoning_efforts {
            if *effort == ReasoningEffort::Minimal || !efforts.insert(effort.as_str()) {
                return Err(SecureClientError::catalog(
                    "model reasoning capabilities are invalid",
                ));
            }
        }

        let provider_url = config
            .endpoint_policy
            .validate_provider_url(&item.base_url)?;
        // Resolve and pin once during validation so a catalog hostname whose
        // current answers include a local address is rejected before the user
        // can select it. Establishment repeats this check and pins its own
        // exact request connection.
        let _ = pinned_client(
            &provider_url,
            &config.endpoint_policy,
            config.attestation_timeout,
            false,
        )
        .await?;

        let mut model = ModelInfo {
            id: item.id,
            label: item.label,
            short_label: item.short_label,
            provider_id: item.provider,
            provider_label: item.provider_label,
            upstream_model: item.model,
            provider_base_url: provider_url.to_string(),
            e2ee_protocol: item.e2ee_protocol,
            e2ee_encryption_version: item.e2ee_encryption_version,
            attestation_protocol: item.attestation_protocol,
            context_window_tokens: item.context_window_tokens,
            max_output_tokens: item.max_output_tokens,
            supported_reasoning_efforts: item.supported_reasoning_efforts,
            ..ModelInfo::default()
        };
        providers.validate_model(&mut model)?;
        output.push(model);
    }
    Ok(output)
}

pub(crate) fn relay_endpoint(base_url: &Url, path: &str) -> Result<Url> {
    let endpoint = format!("{}{}", base_url.as_str().trim_end_matches('/'), path);
    Url::parse(&endpoint).map_err(|_| SecureClientError::configuration("relay endpoint is invalid"))
}

pub(crate) fn error_for_status(status: StatusCode) -> SecureClientError {
    use axiom_inference::ProviderFailureKind;
    let (kind, detail) = match status.as_u16() {
        401 | 403 => (
            ProviderFailureKind::Authentication,
            "Axiom authentication failed",
        ),
        402 => (
            ProviderFailureKind::InsufficientCredit,
            "Axiom credit is exhausted",
        ),
        404 => (
            ProviderFailureKind::ModelUnavailable,
            "selected model is unavailable",
        ),
        408 | 425 | 500..=504 => (
            ProviderFailureKind::Transient,
            "Axiom service is temporarily unavailable",
        ),
        429 => (
            ProviderFailureKind::RateLimited,
            "Axiom request was rate limited",
        ),
        400 | 409 | 415 | 422 => (
            ProviderFailureKind::InvalidRequest,
            "Axiom rejected the request contract",
        ),
        _ => (
            ProviderFailureKind::InvalidResponse,
            "Axiom returned an unexpected status",
        ),
    };
    SecureClientError::new(kind, detail)
}

fn validate_identifier(value: &str, limit: usize, detail: &'static str) -> Result<()> {
    if value.is_empty()
        || value.len() > limit
        || value
            .chars()
            .any(|character| !(character.is_ascii_alphanumeric() || "_.:-".contains(character)))
    {
        Err(SecureClientError::catalog(detail))
    } else {
        Ok(())
    }
}

fn validate_text(value: &str, limit: usize, detail: &'static str) -> Result<()> {
    if value.trim().is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        Err(SecureClientError::catalog(detail))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn config() -> SecureClientConfig {
        let mut config = SecureClientConfig::new("https://api.axiom.stream").unwrap();
        config.endpoint_policy.reject_local_addresses = false;
        config
    }

    fn model(id: &str) -> CatalogModel {
        CatalogModel {
            id: id.into(),
            label: "Model".into(),
            short_label: "Model".into(),
            model: "model/upstream".into(),
            base_url: "https://127.0.0.1/v1".into(),
            provider: "near".into(),
            provider_label: "NEAR".into(),
            e2ee_protocol: "near-v2".into(),
            e2ee_encryption_version: 2,
            attestation_protocol: "near-tdx-nvidia-v1".into(),
            context_window_tokens: 1_000_000,
            max_output_tokens: 8_192,
            supported_reasoning_efforts: vec![ReasoningEffort::Medium],
        }
    }

    #[tokio::test]
    async fn catalog_rejects_duplicates_contract_mismatches_and_impossible_limits() {
        let config = config();
        let providers = ProviderRegistry::production(
            config.clone(),
            Arc::new(ApiCredential::new("test-credential")),
        )
        .unwrap();
        assert!(
            validate_models(vec![model("same"), model("same")], &config, &providers)
                .await
                .is_err()
        );

        let mut invalid = model("invalid");
        invalid.e2ee_protocol = "near-v3".into();
        assert!(
            validate_models(vec![invalid], &config, &providers)
                .await
                .is_err()
        );

        let mut invalid = model("invalid");
        invalid.max_output_tokens = invalid.context_window_tokens + 1;
        assert!(
            validate_models(vec![invalid], &config, &providers)
                .await
                .is_err()
        );
    }

    #[test]
    fn status_mapping_is_stable_and_never_reflects_a_body() {
        assert_eq!(
            error_for_status(StatusCode::PAYMENT_REQUIRED).kind(),
            axiom_inference::ProviderFailureKind::InsufficientCredit
        );
        assert_eq!(
            error_for_status(StatusCode::TOO_MANY_REQUESTS).kind(),
            axiom_inference::ProviderFailureKind::RateLimited
        );
    }
}
