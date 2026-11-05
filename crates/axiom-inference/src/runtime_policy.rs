//! Publisher-authorized backend settings, bounded by installed client capabilities.
use crate::{ImageDimensions, MAX_GENERATED_IMAGE_PIXELS};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRuntimePolicy {
    pub protocol: u32,
    pub minimum_client: u32,
    pub required_features: Vec<String>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub extensions: std::collections::BTreeMap<String, serde_json::Value>,
    pub image: ImageRuntimeSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRuntimeSettings {
    pub default_aspect_ratio: String,
    pub aspect_ratios: Vec<String>,
    pub max_pixels: u64,
    pub max_references: usize,
    pub sampler: String,
    pub cfg: f64,
    pub sigmas: Vec<f64>,
    pub uncensored_models: Vec<String>,
    pub lora: Option<RuntimeLora>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLora {
    pub name: String,
    pub sha256: String,
    pub strength: f64,
    pub patches: usize,
}

impl Default for ImageRuntimePolicy {
    fn default() -> Self {
        Self {
            protocol: 1,
            minimum_client: 1,
            required_features: vec![],
            extensions: std::collections::BTreeMap::default(),
            image: ImageRuntimeSettings {
                default_aspect_ratio: "2:3".into(),
                aspect_ratios: [
                    "1:1", "2:3", "3:2", "3:4", "4:3", "9:16", "16:9", "1:2", "2:1", "9:21", "21:9",
                ]
                .map(str::to_owned)
                .to_vec(),
                max_pixels: MAX_GENERATED_IMAGE_PIXELS,
                max_references: 5,
                sampler: "euler".into(),
                cfg: 1.0,
                sigmas: vec![
                    1.0, 0.978_453, 0.95418, 0.926_626, 0.89508, 0.845_148, 0.704_534, 0.414_568,
                    0.0,
                ],
                uncensored_models: vec![
                    "chutes:qwen3.8-27b".into(),
                    "chutes-preview:qwen3.8-27b".into(),
                ],
                lora: Some(RuntimeLora {
                    name: "uncensored.safetensors".into(),
                    sha256: "83aba822ad8ee10a1e836828f5088259d93bce7b5f3419b46dbc0258c2be46a9"
                        .into(),
                    strength: 0.9,
                    patches: 192,
                }),
            },
        }
    }
}

impl ImageRuntimePolicy {
    pub fn validate(&self) -> Result<(), &'static str> {
        let bad = "Unsupported image runtime policy. Update Axiom before generating.";
        let p = &self.image;
        if self.protocol != 1
            || self.minimum_client != 1
            || !self.required_features.is_empty()
            || self.extensions.len() > 32
            || serde_json::to_vec(&self.extensions).map_or(true, |v| v.len() > 8192)
            || self
                .extensions
                .get("reference_uses_default_size")
                .is_some_and(|v| !v.is_boolean())
            || !(256 * 256..=MAX_GENERATED_IMAGE_PIXELS).contains(&p.max_pixels)
            || p.max_references > 5
            || p.aspect_ratios.is_empty()
            || p.aspect_ratios.len() > 32
            || !p.aspect_ratios.contains(&p.default_aspect_ratio)
            || !["euler", "heun", "dpmpp_2m"].contains(&p.sampler.as_str())
            || !p.cfg.is_finite()
            || !(0.0..=20.0).contains(&p.cfg)
            || !(2..=65).contains(&p.sigmas.len())
            || p.sigmas.first() != Some(&1.0)
            || p.sigmas.last() != Some(&0.0)
            || p.sigmas
                .iter()
                .any(|s| !s.is_finite() || !(0.0..=1.0).contains(s))
            || p.sigmas.windows(2).any(|s| s[0] <= s[1])
            || p.uncensored_models.len() > 32
        {
            return Err(bad);
        }
        let mut seen = std::collections::BTreeSet::new();
        for ratio in &p.aspect_ratios {
            if !seen.insert(ratio) {
                return Err(bad);
            }
            self.dimensions(ratio)?;
        }
        seen.clear();
        for model in &p.uncensored_models {
            if !seen.insert(model)
                || model.len() > 170
                || !model.contains(':')
                || !model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-:._".contains(&b))
            {
                return Err(bad);
            }
        }
        if let Some(lora) = &p.lora
            && (lora.name.len() > 122
                || !lora.name.ends_with(".safetensors")
                || !lora
                    .name
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                || !lora
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                || lora.sha256.len() != 64
                || !lora
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                || !lora.strength.is_finite()
                || !(0.0..=2.0).contains(&lora.strength)
                || !(1..=4096).contains(&lora.patches))
        {
            return Err(bad);
        }
        Ok(())
    }

    #[must_use]
    pub fn references_use_default_size(&self) -> bool {
        self.extensions
            .get("reference_uses_default_size")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    }

    pub fn dimensions(&self, ratio: &str) -> Result<ImageDimensions, &'static str> {
        let bad = "Unsupported image aspect ratio.";
        if !self.image.aspect_ratios.iter().any(|r| r == ratio) {
            return Err(bad);
        }
        let (w, h) = ratio.split_once(':').ok_or(bad)?;
        let parse = |s: &str| -> Result<u64, &'static str> {
            if s.is_empty()
                || s.len() > 2
                || s.starts_with('0')
                || !s.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(bad);
            }
            s.parse().map_err(|_| bad)
        };
        let (mut w, mut h) = (parse(w)?, parse(h)?);
        if w > h * 4 || h > w * 4 {
            return Err(bad);
        }
        let (mut a, mut b) = (w, h);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        w /= a;
        h /= a;
        let scale = (self.image.max_pixels / (1024 * w * h)).isqrt();
        if scale == 0 || w.min(h) * scale * 32 < 256 {
            return Err(bad);
        }
        let size = ImageDimensions {
            width: u32::try_from(w * scale * 32).map_err(|_| bad)?,
            height: u32::try_from(h * scale * 32).map_err(|_| bad)?,
        };
        size.validate().map_err(|_| bad)?;
        Ok(size)
    }

    #[must_use]
    pub fn uncensored(&self, model: &str) -> bool {
        self.image.uncensored_models.iter().any(|m| m == model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_default_size_is_explicit_and_rejects_malformed_policy_values() {
        let mut runtime = ImageRuntimePolicy::default();
        assert!(!runtime.references_use_default_size());
        runtime.extensions.insert(
            "reference_uses_default_size".into(),
            serde_json::json!(true),
        );
        runtime.validate().unwrap();
        assert!(runtime.references_use_default_size());
        runtime.extensions.insert(
            "reference_uses_default_size".into(),
            serde_json::json!("true"),
        );
        assert!(runtime.validate().is_err());
    }
}
