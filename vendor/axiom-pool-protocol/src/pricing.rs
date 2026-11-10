//! Integer-only prices carried in the controller-signed grant.
//!
//! Reasoning tokens are a subset of output tokens, and cached tokens are a
//! subset of input tokens. Neither is added to its parent total a second time.
use crate::types::{Binding, Operation, Usage};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

pub const MAX_MONEY: u64 = 9_007_199_254_740_991;
const MILLION: u128 = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub version: String,
    pub currency: String,
    pub input_microusd_per_million: u64,
    pub cached_input_microusd_per_million: u64,
    pub output_microusd_per_million: u64,
    pub image_microusd_per_megapixel_step: u64,
    pub request_microusd: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Charge {
    pub input_microusd: u64,
    pub output_microusd: u64,
    pub image_microusd: u64,
    pub request_microusd: u64,
    pub total_microusd: u64,
}

fn scaled(units: u128, rate: u64) -> Result<u64> {
    let value = units
        .checked_mul(u128::from(rate))
        .ok_or_else(|| anyhow::anyhow!("price overflow"))?
        .div_ceil(MILLION);
    ensure!(
        value <= u128::from(MAX_MONEY),
        "charge exceeds money bounds"
    );
    Ok(value as u64)
}

impl Price {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.version.is_empty()
                && self.version.len() <= 64
                && self
                    .version
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                && self.currency == "USD",
            "invalid price identity"
        );
        for rate in [
            self.input_microusd_per_million,
            self.cached_input_microusd_per_million,
            self.output_microusd_per_million,
            self.image_microusd_per_megapixel_step,
            self.request_microusd,
        ] {
            ensure!(rate <= MAX_MONEY, "price exceeds money bounds");
        }
        Ok(())
    }

    pub fn charge(&self, binding: &Binding, usage: &Usage) -> Result<Charge> {
        self.validate()?;
        usage.validate(binding)?;
        let input = u128::from(usage.input_tokens - usage.cached_input_tokens)
            * u128::from(self.input_microusd_per_million)
            + u128::from(usage.cached_input_tokens)
                * u128::from(self.cached_input_microusd_per_million);
        let mut charge = Charge {
            input_microusd: u64::try_from(input.div_ceil(MILLION))?,
            output_microusd: scaled(
                u128::from(usage.output_tokens),
                self.output_microusd_per_million,
            )?,
            image_microusd: scaled(
                u128::from(usage.image_pixels) * u128::from(usage.image_steps),
                self.image_microusd_per_megapixel_step,
            )?,
            request_microusd: self.request_microusd,
            total_microusd: 0,
        };
        charge.total_microusd = charge
            .input_microusd
            .checked_add(charge.output_microusd)
            .and_then(|v| v.checked_add(charge.image_microusd))
            .and_then(|v| v.checked_add(charge.request_microusd))
            .filter(|v| *v <= MAX_MONEY)
            .ok_or_else(|| anyhow::anyhow!("charge overflow"))?;
        Ok(charge)
    }

    /// A conservative prepaid bound; it deliberately does not assume cache hits.
    pub fn maximum(&self, binding: &Binding) -> Result<u64> {
        self.validate()?;
        let (input, output, image) = match binding.operation {
            Operation::Chat => (
                scaled(
                    u128::from(binding.limits.context_tokens),
                    self.input_microusd_per_million
                        .max(self.cached_input_microusd_per_million),
                )?,
                scaled(
                    u128::from(binding.limits.output_tokens),
                    self.output_microusd_per_million,
                )?,
                0,
            ),
            Operation::Image => (
                0,
                0,
                scaled(
                    u128::from(binding.limits.image_pixels)
                        * u128::from(binding.limits.image_steps),
                    self.image_microusd_per_megapixel_step,
                )?,
            ),
        };
        input
            .checked_add(output)
            .and_then(|v| v.checked_add(image))
            .and_then(|v| v.checked_add(self.request_microusd))
            .filter(|v| *v <= MAX_MONEY)
            .ok_or_else(|| anyhow::anyhow!("maximum charge overflow"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;

    fn price() -> Price {
        Price {
            version: "staging-1".into(),
            currency: "USD".into(),
            input_microusd_per_million: 250_000,
            cached_input_microusd_per_million: 50_000,
            output_microusd_per_million: 1_000_000,
            image_microusd_per_megapixel_step: 500,
            request_microusd: 0,
        }
    }

    #[test]
    fn cache_discount_and_reasoning_do_not_double_count_tokens() {
        let grant = Fixture::new().grant("account", 1000);
        let usage = Usage {
            input_tokens: 100,
            cached_input_tokens: 40,
            output_tokens: 30,
            reasoning_tokens: 20,
            ..Usage::default()
        };
        let charge = price().charge(&grant.binding, &usage).unwrap();
        assert_eq!(charge.input_microusd, 17);
        assert_eq!(charge.output_microusd, 30);
        assert_eq!(charge.total_microusd, 47);
        assert!(charge.total_microusd <= price().maximum(&grant.binding).unwrap());
    }

    #[test]
    fn image_cost_counts_actual_pixels_and_steps() {
        let mut grant = Fixture::new().grant("account", 1000);
        grant.binding.operation = Operation::Image;
        let usage = Usage {
            image_pixels: 512 * 512,
            image_steps: 8,
            ..Usage::default()
        };
        assert_eq!(
            price()
                .charge(&grant.binding, &usage)
                .unwrap()
                .total_microusd,
            1049
        );
    }

    #[test]
    fn tiny_cost_rounds_up_and_invalid_or_overflowing_meters_fail() {
        let grant = Fixture::new().grant("account", 1000);
        let mut usage = Usage {
            input_tokens: 1,
            ..Usage::default()
        };
        assert_eq!(
            price()
                .charge(&grant.binding, &usage)
                .unwrap()
                .total_microusd,
            1
        );
        usage.cached_input_tokens = 2;
        assert!(price().charge(&grant.binding, &usage).is_err());
        usage.cached_input_tokens = 0;
        usage.reasoning_tokens = 1;
        assert!(price().charge(&grant.binding, &usage).is_err());
        let mut huge = price();
        huge.input_microusd_per_million = MAX_MONEY;
        huge.request_microusd = MAX_MONEY;
        assert!(huge.maximum(&grant.binding).is_err());
    }
}
