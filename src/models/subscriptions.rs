//! `GET /rest/subscriptions` payload.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::UserId;

/// Top-level response.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SubscriptionsResponse {
    pub subscriptions: Vec<Subscription>,
}

/// Per-subscription entry. Upstream carries either a `stripe` or an `x` block
/// (X / Twitter entitlement); we keep both as raw JSON because they are not
/// on the hot path.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Subscription {
    #[serde(rename = "xaiUserId")]
    pub xai_user_id: UserId,
    pub tier: Tier,
    pub status: SubscriptionStatus,
    #[serde(rename = "createTime")]
    pub create_time: String,
    #[serde(rename = "modTime")]
    pub mod_time: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stripe: Option<serde_json::Value>,
    #[serde(default, rename = "x", skip_serializing_if = "Option::is_none")]
    pub x_twitter: Option<serde_json::Value>,
}

/// Observed values: `SUBSCRIPTION_TIER_GROK_PRO`, `SUBSCRIPTION_TIER_X_PREMIUM`.
/// Other strings are preserved verbatim.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Tier {
    SubscriptionTierGrokPro,
    SubscriptionTierXPremium,
    #[serde(untagged)]
    Other(String),
}

/// Observed value: `SUBSCRIPTION_STATUS_ACTIVE`.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SubscriptionStatus {
    SubscriptionStatusActive,
    SubscriptionStatusInactive,
    SubscriptionStatusCancelled,
    #[serde(untagged)]
    Other(String),
}
