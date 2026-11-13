//! Billing v7 carries distinct allowance credits and fixed-USD purchased credit.

use agent_client_protocol::{JsonRpcRequest, JsonRpcResponse};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreditSummary {
    pub format: String,
    pub enabled: bool,
    pub scale: String,
    pub revision: String,
    pub policy_version: String,
    pub reference_microusd_per_credit: u32,
    pub default_multiplier_ppm: u32,
    pub paid_fallback: bool,
    pub payment_review_required: bool,
    pub purchased: PurchasedCredits,
    pub trial: TrialCredits,
    pub allowance: WeeklyCredits,
    pub subscription: Option<CreditSubscription>,
    pub capabilities: CreditCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PurchasedCredits {
    pub currency: String,
    pub scale: String,
    pub posted_units: String,
    pub held_units: String,
    pub available_units: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrialCredits {
    pub posted_units: String,
    pub held_units: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WeeklyCredits {
    pub limit_units: String,
    pub remaining_units: String,
    pub held_units: String,
    pub generation: Option<String>,
    pub resets_at: Option<String>,
    pub effective_until: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreditSubscription {
    pub id: String,
    pub plan_key: String,
    pub state: String,
    pub paid_through_at: Option<String>,
    pub cancel_at_period_end: bool,
    pub pending_plan_key: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Independent server capabilities in the wire contract.
pub struct CreditCapabilities {
    pub subscriptions: bool,
    pub auto_topup: bool,
    pub card: bool,
    pub crypto: bool,
    pub admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionPlan {
    pub key: String,
    pub name: String,
    pub amount_cents: u64,
    pub weekly_units: String,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionPlans {
    pub enabled: bool,
    pub currency: String,
    pub billing_interval: String,
    pub allowance_interval_days: u32,
    pub plans: Vec<SubscriptionPlan>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SavedBillingCard {
    pub id: String,
    pub brand: String,
    pub last4: String,
    pub exp_month: u32,
    pub exp_year: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SavedBillingCards {
    pub cards: Vec<SavedBillingCard>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BillingOperation {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub url: Option<String>,
    pub amount_cents: u64,
    pub card: Option<SavedBillingCard>,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BillingPortal {
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutomaticTopup {
    pub currency: String,
    pub scale: String,
    pub available: bool,
    pub enabled: bool,
    pub version: String,
    pub threshold_units: String,
    pub amount_cents: u64,
    pub monthly_cap_cents: u64,
    pub payment_method_id: Option<String>,
    pub paused_reason: Option<String>,
    pub cap_month: String,
    pub used_cents: u64,
    pub consent_version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AutomaticTopupSettings {
    pub enabled: bool,
    pub version: String,
    pub threshold_units: String,
    pub amount_cents: u64,
    pub monthly_cap_cents: u64,
    pub payment_method_id: Option<String>,
    pub consent_version: String,
    pub consent_accepted: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CardFundingOptions {
    pub enabled: bool,
    pub currency: String,
    pub min_amount_microusd: u64,
    pub max_amount_microusd: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CardFundingPayment {
    pub id: String,
    pub amount_microusd: u64,
    pub credited_microusd: u64,
    pub refunded_microusd: u64,
    pub status: String,
    pub checkout_url: Option<String>,
    pub expires_at: String,
    pub created_at: String,
    pub review_required: bool,
    pub cancel_requested: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreditModelUsage {
    pub provider: String,
    pub model_id: String,
    pub units: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreditUsage {
    pub period: String,
    pub scale: String,
    pub total_units: String,
    pub allocation: CreditUsageAllocation,
    pub models: Vec<CreditModelUsage>,
    pub purchased: PurchasedUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PurchasedUsage {
    pub currency: String,
    pub scale: String,
    pub total_units: String,
    pub models: Vec<CreditModelUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreditUsageAllocation {
    pub subscription_units: String,
    pub trial_units: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreditModelPrice {
    pub provider: String,
    pub model_id: String,
    pub version: String,
    pub multiplier_ppm: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CreditPrices {
    pub scale: String,
    pub reference_microusd_per_credit: u32,
    pub default_version: String,
    pub default_multiplier_ppm: u32,
    pub overrides: Vec<CreditModelPrice>,
}

/// A finite operation set. Renderers cannot select arbitrary URLs, headers or principals.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CreditBillingAction {
    Summary,
    Plans,
    Prices,
    Usage {
        period: Option<String>,
        timezone: Option<String>,
    },
    Checkout {
        id: String,
        plan_key: String,
        catalog_version: String,
    },
    Change {
        id: String,
        plan_key: String,
        catalog_version: String,
    },
    Portal,
    SetupCard {
        id: String,
    },
    Cards,
    AutoTopup,
    ConfigureAutoTopup {
        settings: AutomaticTopupSettings,
    },
    Fallback {
        enabled: bool,
    },
    Operations,
    Operation {
        id: String,
    },
    Check {
        id: String,
    },
    CancelTopup {
        id: String,
    },
    CardOptions,
    CreateCardPayment {
        id: String,
        amount_microusd: u64,
    },
    CardPayments,
    CheckCardPayment {
        id: String,
    },
    CancelCardPayment {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum CreditBillingResult {
    Summary(Option<Box<CreditSummary>>),
    Plans(SubscriptionPlans),
    Prices(CreditPrices),
    Usage(CreditUsage),
    Operation(BillingOperation),
    Portal(BillingPortal),
    Cards(SavedBillingCards),
    AutoTopup(AutomaticTopup),
    Operations(Vec<BillingOperation>),
    CardOptions(CardFundingOptions),
    CardPayment(CardFundingPayment),
    CardPayments(Vec<CardFundingPayment>),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, JsonRpcRequest)]
#[request(method = "_axiom/billing/credits", response = CreditBillingResponse)]
#[serde(deny_unknown_fields)]
pub struct CreditBillingRequest {
    pub action: CreditBillingAction,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, JsonRpcResponse)]
#[serde(deny_unknown_fields)]
pub struct CreditBillingResponse {
    pub result: CreditBillingResult,
}
