use std::collections::BTreeSet;

use axiom_inference::{ModelInfo, ReasoningEffort};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    ApiCredential, ProviderRegistry, Result, SecureClientConfig, SecureClientError,
    http::{bounded_body, pinned_client},
};

const MAX_MODELS: usize = 512;
const MAX_MODEL_TOKENS: u32 = 16 * 1024 * 1024;

#[derive(Clone, Deserialize, Serialize)]
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
    let wire: Vec<Value> = serde_json::from_slice(&body)
        .map_err(|_| SecureClientError::catalog("model catalog schema is invalid"))?;
    validate_models(wire, config, providers)
}

fn validate_models(
    wire: Vec<Value>,
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
    for raw in wire {
        let item: CatalogModel = if let Ok(item) = serde_json::from_value(raw) {
            item
        } else {
            tracing::warn!("malformed model catalog entry was quarantined");
            continue;
        };
        if let Err(error) = validate_identifier(&item.id, 80, "model ID is invalid") {
            tracing::warn!(
                "invalid model catalog entry was quarantined: {}",
                error.safe_detail()
            );
            continue;
        }
        if !ids.insert(item.id.clone()) {
            return Err(SecureClientError::catalog(
                "model catalog contains a duplicate model ID",
            ));
        }
        if validate_identifier(&item.provider, 64, "provider ID is invalid").is_err() {
            tracing::warn!(model = %item.id, "model with invalid provider ID was quarantined");
            continue;
        }
        if !providers.contains(&item.provider) {
            tracing::warn!(
                provider = %item.provider,
                model = %item.id,
                "model omitted because this client has no local provider engine"
            );
            continue;
        }
        let metadata_valid = validate_text(&item.label, 200, "model label is invalid").is_ok()
            && validate_text(&item.short_label, 100, "model short label is invalid").is_ok()
            && validate_text(&item.model, 256, "upstream model name is invalid").is_ok()
            && validate_text(&item.provider_label, 100, "provider label is invalid").is_ok()
            && validate_identifier(&item.e2ee_protocol, 64, "E2EE protocol ID is invalid").is_ok()
            && validate_identifier(
                &item.attestation_protocol,
                64,
                "attestation protocol ID is invalid",
            )
            .is_ok();
        if !metadata_valid {
            tracing::warn!(model = %item.id, "model with invalid metadata was quarantined");
            continue;
        }
        if item.context_window_tokens == 0
            || item.max_output_tokens == 0
            || item.context_window_tokens > MAX_MODEL_TOKENS
            || item.max_output_tokens > item.context_window_tokens
        {
            tracing::warn!(model = %item.id, "model with invalid token limits was quarantined");
            continue;
        }
        let mut efforts = BTreeSet::new();
        for effort in &item.supported_reasoning_efforts {
            if *effort == ReasoningEffort::Minimal || !efforts.insert(effort.as_str()) {
                tracing::warn!(model = %item.id, "model with invalid reasoning metadata was quarantined");
                efforts.clear();
                break;
            }
        }
        if efforts.len() != item.supported_reasoning_efforts.len() {
            continue;
        }

        let provider_url = config
            .endpoint_policy
            .validate_provider_url(&item.base_url)?;
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
        if let Err(error) = providers.validate_model(&mut model) {
            tracing::warn!(
                model = %model.id,
                provider = %model.provider_id,
                detail = error.safe_detail(),
                "model with an unsupported provider contract was quarantined"
            );
            continue;
        }
        output.push(model);
    }
    if output.is_empty() {
        return Err(SecureClientError::capability(
            "model catalog contains no providers supported by this client",
        ));
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

    fn wire(model: CatalogModel) -> Value {
        serde_json::to_value(model).unwrap()
    }

    #[test]
    fn catalog_rejects_duplicates_contract_mismatches_and_impossible_limits() {
        let config = config();
        let providers = ProviderRegistry::production(
            config.clone(),
            Arc::new(ApiCredential::new("test-credential")),
        )
        .unwrap();
        assert!(
            validate_models(
                vec![wire(model("same")), wire(model("same"))],
                &config,
                &providers,
            )
            .is_err()
        );

        let mut invalid = model("invalid");
        invalid.e2ee_protocol = "near-v3".into();
        assert!(validate_models(vec![wire(invalid)], &config, &providers).is_err());

        let mut invalid = model("invalid");
        invalid.max_output_tokens = invalid.context_window_tokens + 1;
        assert!(validate_models(vec![wire(invalid)], &config, &providers).is_err());
    }

    #[test]
    fn catalog_omits_unknown_providers_without_weakening_known_contracts() {
        let config = config();
        let providers = ProviderRegistry::production(
            config.clone(),
            Arc::new(ApiCredential::new("test-credential")),
        )
        .unwrap();
        let mut unknown = model("future-model");
        unknown.provider = "future-provider".into();
        unknown.base_url = "http://127.0.0.1/this-must-not-be-resolved".into();

        let supported = validate_models(
            vec![wire(model("near-model")), wire(unknown.clone())],
            &config,
            &providers,
        )
        .unwrap();
        assert_eq!(supported.len(), 1);
        assert_eq!(supported[0].id, "near-model");

        assert!(validate_models(vec![wire(unknown)], &config, &providers).is_err());

        let mut mismatched_near = model("bad-near");
        mismatched_near.e2ee_protocol = "near-v3".into();
        assert!(validate_models(vec![wire(mismatched_near)], &config, &providers).is_err());
    }

    #[test]
    fn catalog_rejects_unknown_fields_and_quarantines_malformed_neighbors() {
        let config = config();
        let providers = ProviderRegistry::production(
            config.clone(),
            Arc::new(ApiCredential::new("test-credential")),
        )
        .unwrap();
        let mut unknown = wire(model("unknown-contract"));
        unknown.as_object_mut().unwrap().insert(
            "future_server_metadata".into(),
            serde_json::json!({"version": 2}),
        );
        let models = validate_models(
            vec![
                serde_json::json!({"broken": true}),
                unknown.clone(),
                wire(model("supported")),
            ],
            &config,
            &providers,
        )
        .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "supported");
        assert!(validate_models(vec![unknown], &config, &providers).is_err());
    }

    #[test]
    fn catalog_rejects_contract_negotiation_fields_and_newer_contracts() {
        let config = config();
        let providers = ProviderRegistry::production(
            config.clone(),
            Arc::new(ApiCredential::new("test-credential")),
        )
        .unwrap();
        let mut forward = model("newer");
        forward.e2ee_protocol = "near-v3".into();
        forward.e2ee_encryption_version = 3;
        forward.attestation_protocol = "near-tdx-nvidia-v2".into();
        assert!(validate_models(vec![wire(forward)], &config, &providers).is_err());

        let mut ambiguous = wire(model("ambiguous"));
        ambiguous.as_object_mut().unwrap().insert(
            "e2ee_contracts".into(),
            serde_json::json!([{
                "e2ee_protocol": "near-v2",
                "e2ee_encryption_version": 2,
                "attestation_protocol": "near-tdx-nvidia-v1"
            }]),
        );
        assert!(validate_models(vec![ambiguous], &config, &providers).is_err());
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
