use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use axiom_inference::{MAX_MODEL_TAGS, ModelInfo, ModelTag, ModelTagTone, ReasoningEffort};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    ApiCredential, ProviderRegistry, Result, SecureClientConfig, SecureClientError,
    http::{bounded_body, relay_client},
    provider::ProviderContract,
};

const MAX_MODELS: usize = 512;
const MAX_E2EE_CONTRACTS_PER_MODEL: usize = 8;
const MAX_REASONING_EFFORTS_PER_MODEL: usize = 16;
const MAX_MODEL_TOKENS: u32 = 16 * 1024 * 1024;
// $1,000,000 per million tokens. This is intentionally far above real catalog
// rates while remaining well inside JavaScript's exact integer range.
const MAX_MODEL_PRICE_MICROUSD_PER_MILLION_TOKENS: u64 = 1_000_000_000_000;
const MAX_CATALOG_TAG_ENTRIES: usize = 64;
const MAX_TAG_ID_BYTES: usize = 32;
const MAX_TAG_LABEL_CHARS: usize = 32;
const MAX_TAG_POSITION: u64 = 10_000;

#[derive(Clone, Deserialize, Serialize)]
struct CatalogE2eeContract {
    e2ee_protocol: String,
    e2ee_encryption_version: u16,
    attestation_protocol: String,
    #[serde(default)]
    preferred: bool,
    #[serde(default)]
    retire_after_unix_seconds: Option<u64>,
}

#[derive(Clone, Deserialize, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent advertised model capabilities"
)]
struct CatalogModel {
    #[serde(default)]
    relay_contract_version: u32,
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
    input_price_microusd_per_million_tokens: Option<u64>,
    #[serde(default)]
    output_price_microusd_per_million_tokens: Option<u64>,
    #[serde(default)]
    supported_reasoning_efforts: Vec<String>,
    #[serde(default)]
    supported_thinking_modes: Vec<axiom_inference::ThinkingMode>,
    #[serde(default)]
    reasoning_replay: bool,
    #[serde(default)]
    supports_tools: bool,
    #[serde(default)]
    supports_parallel_tools: bool,
    #[serde(default)]
    supports_images: bool,
    #[serde(default)]
    file_mime_types: Vec<String>,
    #[serde(default)]
    reasoning_parameters: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    thinking_parameters: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    e2ee_contracts: Vec<CatalogE2eeContract>,
    /// Catalog tags. Kept as raw JSON so a malformed tag never
    /// quarantines its model; see `catalog_tags`.
    #[serde(default)]
    tags: Value,
}

#[derive(Deserialize)]
struct CatalogTag {
    id: String,
    label: String,
    #[serde(default)]
    tone: Option<String>,
    #[serde(default)]
    position: Option<u64>,
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
    let client = relay_client(config, &endpoint, config.request_timeout).await?;
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
        let status = response.status();
        // Only the bounded operational code is trusted here. Never display
        // backend error text or navigate to a URL supplied in an error body.
        if status == StatusCode::FORBIDDEN
            && let Ok(body) =
                bounded_body(response, config.limits.model_catalog_bytes.min(8192)).await
            && let Ok(value) = serde_json::from_slice::<Value>(&body)
            && value
                .get("detail")
                .and_then(|detail| detail.get("code"))
                .and_then(Value::as_str)
                == Some("consent_required")
        {
            return Err(SecureClientError::new(
                axiom_inference::ProviderFailureKind::Authentication,
                "Review the current policies on your Axiom account page to restore model access",
            ));
        }
        return Err(error_for_status(status));
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
        let item: CatalogModel = match serde_json::from_value(raw) {
            Ok(item) => item,
            Err(error) => {
                tracing::warn!(%error, "malformed model catalog entry was quarantined");
                continue;
            }
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
        let Some(contract_candidates) = contract_candidates(&item) else {
            tracing::warn!(
                model = %item.id,
                "model with invalid E2EE contract advertisements was quarantined"
            );
            continue;
        };
        let Some(selected_contract) =
            providers.select_contract(&item.provider, &contract_candidates)
        else {
            tracing::warn!(
                model = %item.id,
                provider = %item.provider,
                "model omitted because no advertised E2EE contract is supported locally"
            );
            continue;
        };
        if item.context_window_tokens == 0
            || item.max_output_tokens == 0
            || item.context_window_tokens > MAX_MODEL_TOKENS
            || item.max_output_tokens > item.context_window_tokens
        {
            tracing::warn!(model = %item.id, "model with invalid token limits was quarantined");
            continue;
        }
        match (
            item.input_price_microusd_per_million_tokens,
            item.output_price_microusd_per_million_tokens,
        ) {
            (None, None) => {}
            (Some(input), Some(output))
                if input > 0
                    && output > 0
                    && input <= MAX_MODEL_PRICE_MICROUSD_PER_MILLION_TOKENS
                    && output <= MAX_MODEL_PRICE_MICROUSD_PER_MILLION_TOKENS => {}
            _ => {
                tracing::warn!(model = %item.id, "model with invalid pricing metadata was quarantined");
                continue;
            }
        }
        if item.relay_contract_version != 2 {
            return Err(SecureClientError::capability(
                "The backend does not support this client\'s inference contract. Update the backend before sending messages.",
            ));
        }
        if item.supported_reasoning_efforts.len() > MAX_REASONING_EFFORTS_PER_MODEL {
            tracing::warn!(model = %item.id, "model with excessive reasoning metadata was quarantined");
            continue;
        }
        let mut effort_names = BTreeSet::new();
        let mut supported_reasoning_efforts = Vec::new();
        let mut invalid_reasoning = false;
        for effort in &item.supported_reasoning_efforts {
            if effort.len() > 32 || effort.chars().any(char::is_control) {
                tracing::warn!(model = %item.id, "model with invalid reasoning metadata was quarantined");
                invalid_reasoning = true;
                break;
            }
            let Some(parsed) = parse_reasoning_effort(effort) else {
                tracing::debug!(model = %item.id, effort, "unknown reasoning effort was ignored");
                continue;
            };
            if !effort_names.insert(parsed.as_str()) {
                invalid_reasoning = true;
                break;
            }
            supported_reasoning_efforts.push(parsed);
        }
        if invalid_reasoning {
            continue;
        }

        let provider_url = config
            .endpoint_policy
            .validate_provider_url(&item.base_url)?;
        let tags = catalog_tags(&item.id, &item.tags);
        let mut model = ModelInfo {
            id: item.id,
            label: item.label,
            short_label: item.short_label,
            provider_id: item.provider,
            provider_label: item.provider_label,
            upstream_model: item.model,
            provider_base_url: provider_url.to_string(),
            e2ee_protocol: selected_contract.e2ee_protocol,
            e2ee_encryption_version: selected_contract.e2ee_encryption_version,
            attestation_protocol: selected_contract.attestation_protocol,
            context_window_tokens: item.context_window_tokens,
            max_output_tokens: item.max_output_tokens,
            input_price_microusd_per_million_tokens: item.input_price_microusd_per_million_tokens,
            output_price_microusd_per_million_tokens: item.output_price_microusd_per_million_tokens,
            supported_reasoning_efforts,
            supported_thinking_modes: item.supported_thinking_modes,
            reasoning_replay: item.reasoning_replay,
            supports_tools: item.supports_tools,
            supports_parallel_tools: item.supports_parallel_tools,
            supports_images: item.supports_images,
            file_mime_types: item.file_mime_types,
            reasoning_parameters: item.reasoning_parameters,
            thinking_parameters: item.thinking_parameters,
            tags,
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

/// Tags filter the picker and the `uncensored` tag selects the uncensored image
/// adapter. Malformed entries are dropped one by one and never affect the
/// model's provider contract or availability.
fn catalog_tags(model_id: &str, raw: &Value) -> Vec<ModelTag> {
    let entries = match raw {
        Value::Null => return Vec::new(),
        Value::Array(entries) if entries.len() <= MAX_CATALOG_TAG_ENTRIES => entries,
        _ => {
            tracing::debug!(model = model_id, "invalid model tag list was ignored");
            return Vec::new();
        }
    };
    let mut seen = BTreeSet::new();
    let mut tags = Vec::new();
    for entry in entries {
        let Ok(tag) = CatalogTag::deserialize(entry) else {
            tracing::debug!(model = model_id, "malformed model tag was ignored");
            continue;
        };
        let label = tag.label.trim();
        let valid = valid_tag_id(&tag.id)
            && !label.is_empty()
            && label.chars().count() <= MAX_TAG_LABEL_CHARS
            && !label.chars().any(char::is_control)
            && tag
                .position
                .is_none_or(|position| position <= MAX_TAG_POSITION);
        if !valid || !seen.insert(tag.id.clone()) {
            tracing::debug!(
                model = model_id,
                "invalid or duplicate model tag was ignored"
            );
            continue;
        }
        tags.push(ModelTag {
            label: label.to_owned(),
            tone: tag
                .tone
                .as_deref()
                .and_then(ModelTagTone::parse)
                .unwrap_or_default(),
            position: u16::try_from(tag.position.unwrap_or(0)).unwrap_or(u16::MAX),
            id: tag.id,
        });
    }
    tags.sort_by(|left, right| {
        left.position
            .cmp(&right.position)
            .then_with(|| left.id.cmp(&right.id))
    });
    tags.truncate(MAX_MODEL_TAGS);
    tags
}

fn valid_tag_id(value: &str) -> bool {
    value.len() <= MAX_TAG_ID_BYTES
        && value
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn contract_candidates(item: &CatalogModel) -> Option<Vec<ProviderContract>> {
    let top_level = ProviderContract {
        e2ee_protocol: item.e2ee_protocol.clone(),
        e2ee_encryption_version: item.e2ee_encryption_version,
        attestation_protocol: item.attestation_protocol.clone(),
    };
    if item.e2ee_contracts.is_empty() {
        return Some(vec![top_level]);
    }
    if item.e2ee_contracts.len() > MAX_E2EE_CONTRACTS_PER_MODEL {
        return None;
    }

    let mut seen = BTreeSet::new();
    let mut top_level_advertised = false;
    let mut preferred_contracts = 0usize;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let mut candidates = Vec::with_capacity(item.e2ee_contracts.len());
    for contract in &item.e2ee_contracts {
        if validate_identifier(
            &contract.e2ee_protocol,
            64,
            "advertised E2EE protocol ID is invalid",
        )
        .is_err()
            || validate_identifier(
                &contract.attestation_protocol,
                64,
                "advertised attestation protocol ID is invalid",
            )
            .is_err()
            || contract.e2ee_encryption_version == 0
            || contract.retire_after_unix_seconds == Some(0)
            || !seen.insert((
                contract.e2ee_protocol.as_str(),
                contract.e2ee_encryption_version,
                contract.attestation_protocol.as_str(),
            ))
        {
            return None;
        }

        let is_top_level = contract.e2ee_protocol == item.e2ee_protocol
            && contract.e2ee_encryption_version == item.e2ee_encryption_version
            && contract.attestation_protocol == item.attestation_protocol;
        top_level_advertised |= is_top_level;
        if contract.preferred {
            preferred_contracts += 1;
            if !is_top_level || preferred_contracts > 1 {
                return None;
            }
        }
        if contract
            .retire_after_unix_seconds
            .is_none_or(|retire_after| retire_after > now)
        {
            let candidate = ProviderContract {
                e2ee_protocol: contract.e2ee_protocol.clone(),
                e2ee_encryption_version: contract.e2ee_encryption_version,
                attestation_protocol: contract.attestation_protocol.clone(),
            };
            if is_top_level {
                candidates.insert(0, candidate);
            } else {
                candidates.push(candidate);
            }
        }
    }
    top_level_advertised.then_some(candidates)
}

fn parse_reasoning_effort(value: &str) -> Option<ReasoningEffort> {
    match value {
        "minimal" => Some(ReasoningEffort::Minimal),
        "low" => Some(ReasoningEffort::Low),
        "medium" => Some(ReasoningEffort::Medium),
        "high" => Some(ReasoningEffort::High),
        "xhigh" => Some(ReasoningEffort::ExtraHigh),
        _ => None,
    }
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
            crate::INSUFFICIENT_CREDIT_MESSAGE,
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
            "Axiom is rate limited. Wait before trying again.",
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
            relay_contract_version: 2,
            supported_thinking_modes: Vec::new(),
            reasoning_replay: false,
            supports_tools: true,
            supports_parallel_tools: true,
            supports_images: false,
            file_mime_types: Vec::new(),
            reasoning_parameters: std::collections::BTreeMap::default(),
            thinking_parameters: std::collections::BTreeMap::default(),
            id: id.into(),
            label: "Model".into(),
            short_label: "Model".into(),
            model: "model/upstream".into(),
            base_url: "https://127.0.0.1/v1".into(),
            provider: "near".into(),
            provider_label: "NEAR".into(),
            e2ee_protocol: "near-v3".into(),
            e2ee_encryption_version: 2,
            attestation_protocol: "near-tdx-nvidia-v2".into(),
            context_window_tokens: 1_000_000,
            max_output_tokens: 8_192,
            input_price_microusd_per_million_tokens: Some(440_000),
            output_price_microusd_per_million_tokens: Some(1_320_000),
            supported_reasoning_efforts: vec!["medium".into()],
            e2ee_contracts: Vec::new(),
            tags: Value::Null,
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

        let mut legacy = model("legacy");
        legacy.relay_contract_version = 0;
        assert!(validate_models(vec![wire(legacy)], &config, &providers).is_err());

        let mut invalid = model("invalid");
        invalid.e2ee_protocol = "near-v2".into();
        assert!(validate_models(vec![wire(invalid)], &config, &providers).is_err());

        let mut invalid = model("invalid");
        invalid.max_output_tokens = invalid.context_window_tokens + 1;
        assert!(validate_models(vec![wire(invalid)], &config, &providers).is_err());

        let mut invalid = model("invalid-price");
        invalid.output_price_microusd_per_million_tokens = None;
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
        mismatched_near.e2ee_protocol = "near-v2".into();
        assert!(validate_models(vec![wire(mismatched_near)], &config, &providers).is_err());
    }

    #[test]
    fn catalog_accepts_additive_fields_and_quarantines_malformed_neighbors() {
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
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "unknown-contract");
        assert_eq!(models[1].id, "supported");
        assert_eq!(
            validate_models(vec![unknown], &config, &providers)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn catalog_tags_are_sorted_bounded_and_never_quarantine_their_model() {
        let config = config();
        let providers = ProviderRegistry::production(
            config.clone(),
            Arc::new(ApiCredential::new("test-credential")),
        )
        .unwrap();
        let mut tagged = model("tagged");
        tagged.tags = serde_json::json!([
            {"id": "new", "label": "New", "tone": "orange", "position": 1000},
            {"id": "cheap", "label": " Cheap ", "tone": "green", "position": 10},
            {"id": "Bad ID", "label": "Bad", "position": 1},
            {"id": "cheap", "label": "Duplicate", "position": 0},
            {"id": "empty", "label": "   "},
            {"id": "control", "label": "Line\nbreak"},
            {"id": "far", "label": "Far", "position": 10_001},
            {"id": "negative", "label": "Negative", "position": -1},
            "cheap",
            {"id": "fast", "label": "Fast", "tone": "neon", "position": 20},
            {"id": "future", "label": "Future", "position": 30, "icon": "spark"}
        ]);
        let mut malformed_list = model("malformed-list");
        malformed_list.tags = serde_json::json!({"cheap": true});
        let mut many = model("many");
        many.tags = Value::Array(
            (0..20)
                .rev()
                .map(|index| serde_json::json!({"id": format!("tag-{index:02}"), "label": "Tag", "position": index}))
                .collect(),
        );

        let mut legacy = wire(model("legacy-backend"));
        legacy.as_object_mut().unwrap().remove("tags");
        let models = validate_models(
            vec![wire(tagged), wire(malformed_list), wire(many), legacy],
            &config,
            &providers,
        )
        .unwrap();
        assert_eq!(models.len(), 4);
        let summary = models[0]
            .tags
            .iter()
            .map(|tag| (tag.id.as_str(), tag.label.as_str(), tag.tone, tag.position))
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            vec![
                ("cheap", "Cheap", ModelTagTone::Green, 10),
                ("fast", "Fast", ModelTagTone::Gray, 20),
                ("future", "Future", ModelTagTone::Gray, 30),
                ("new", "New", ModelTagTone::Orange, 1000),
            ]
        );
        assert!(models[1].tags.is_empty());
        assert_eq!(models[2].tags.len(), MAX_MODEL_TAGS);
        assert_eq!(models[2].tags[0].id, "tag-00");
        assert_eq!(models[2].tags[MAX_MODEL_TAGS - 1].id, "tag-15");
        assert!(models[3].tags.is_empty());
    }

    #[test]
    fn catalog_negotiates_the_newest_locally_supported_advertised_contract() {
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

        let mut advertised = model("advertised");
        advertised.e2ee_protocol = "near-v3".into();
        advertised.e2ee_encryption_version = 3;
        advertised.attestation_protocol = "near-tdx-nvidia-v2".into();
        advertised.e2ee_contracts = vec![
            CatalogE2eeContract {
                e2ee_protocol: "near-v3".into(),
                e2ee_encryption_version: 3,
                attestation_protocol: "near-tdx-nvidia-v2".into(),
                preferred: true,
                retire_after_unix_seconds: None,
            },
            CatalogE2eeContract {
                e2ee_protocol: "near-v3".into(),
                e2ee_encryption_version: 2,
                attestation_protocol: "near-tdx-nvidia-v2".into(),
                preferred: false,
                retire_after_unix_seconds: None,
            },
        ];
        let supported = validate_models(vec![wire(advertised)], &config, &providers).unwrap();
        assert_eq!(supported.len(), 1);
        assert_eq!(supported[0].e2ee_protocol, "near-v3");
        assert_eq!(supported[0].e2ee_encryption_version, 2);

        // Preserve the old server default for existing clients, while this
        // native engine opts into the fully implemented multi-worker contract.
        let mut gateway = model("gateway");
        gateway.e2ee_protocol = "near-v3".into();
        gateway.e2ee_encryption_version = 2;
        gateway.attestation_protocol = "near-tdx-nvidia-gateway-v1".into();
        let legacy = validate_models(vec![wire(gateway.clone())], &config, &providers).unwrap();
        assert_eq!(legacy[0].attestation_protocol, "near-tdx-nvidia-gateway-v1");
        gateway.e2ee_contracts = vec![
            CatalogE2eeContract {
                e2ee_protocol: "near-v3".into(),
                e2ee_encryption_version: 2,
                attestation_protocol: "near-tdx-nvidia-gateway-v1".into(),
                preferred: true,
                retire_after_unix_seconds: None,
            },
            CatalogE2eeContract {
                e2ee_protocol: "near-v3".into(),
                e2ee_encryption_version: 2,
                attestation_protocol: "near-tdx-nvidia-gateway-v2".into(),
                preferred: false,
                retire_after_unix_seconds: None,
            },
        ];
        let upgraded = validate_models(vec![wire(gateway)], &config, &providers).unwrap();
        assert_eq!(
            upgraded[0].attestation_protocol,
            "near-tdx-nvidia-gateway-v2"
        );

        let mut retired = model("retired");
        retired.e2ee_protocol = "near-v3".into();
        retired.e2ee_encryption_version = 3;
        retired.attestation_protocol = "near-tdx-nvidia-v2".into();
        retired.e2ee_contracts = vec![
            CatalogE2eeContract {
                e2ee_protocol: "near-v3".into(),
                e2ee_encryption_version: 3,
                attestation_protocol: "near-tdx-nvidia-v2".into(),
                preferred: true,
                retire_after_unix_seconds: None,
            },
            CatalogE2eeContract {
                e2ee_protocol: "near-v3".into(),
                e2ee_encryption_version: 2,
                attestation_protocol: "near-tdx-nvidia-v2".into(),
                preferred: false,
                retire_after_unix_seconds: Some(1),
            },
        ];
        assert!(validate_models(vec![wire(retired)], &config, &providers).is_err());

        let mut contradictory = model("contradictory");
        contradictory.e2ee_contracts = vec![CatalogE2eeContract {
            e2ee_protocol: "near-v3".into(),
            e2ee_encryption_version: 3,
            attestation_protocol: "near-tdx-nvidia-v2".into(),
            preferred: true,
            retire_after_unix_seconds: None,
        }];
        assert!(validate_models(vec![wire(contradictory)], &config, &providers).is_err());
    }

    #[test]
    fn catalog_ignores_unknown_reasoning_values_but_keeps_known_values() {
        let config = config();
        let providers = ProviderRegistry::production(
            config.clone(),
            Arc::new(ApiCredential::new("test-credential")),
        )
        .unwrap();
        let mut item = model("reasoning-rollout");
        item.supported_reasoning_efforts = vec!["medium".into(), "ultra".into()];
        let models = validate_models(vec![wire(item)], &config, &providers).unwrap();
        assert_eq!(
            models[0].supported_reasoning_efforts,
            vec![ReasoningEffort::Medium]
        );
    }

    #[test]
    fn status_mapping_is_stable_and_never_reflects_a_body() {
        assert_eq!(
            error_for_status(StatusCode::PAYMENT_REQUIRED).kind(),
            axiom_inference::ProviderFailureKind::InsufficientCredit
        );
        assert_eq!(
            error_for_status(StatusCode::PAYMENT_REQUIRED).safe_detail(),
            crate::INSUFFICIENT_CREDIT_MESSAGE
        );
        assert_eq!(
            error_for_status(StatusCode::TOO_MANY_REQUESTS).kind(),
            axiom_inference::ProviderFailureKind::RateLimited
        );
        let limited = error_for_status(StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            limited.safe_detail(),
            "Axiom is rate limited. Wait before trying again."
        );
        assert!(limited.retryable());
        assert!(!limited.requires_reattest_and_reencrypt());
    }
}
