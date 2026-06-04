//! `GET /rest/subscriptions` payload.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::UserId;

/// Top-level response.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SubscriptionsResponse {
    pub subscriptions: Vec<Subscription>,
}

impl SubscriptionsResponse {
    /// Prefer an active entitlement when Grok returns historical records too.
    #[must_use]
    pub fn primary(&self) -> Option<&Subscription> {
        self.subscriptions
            .iter()
            .find(|subscription| subscription.is_active())
            .or_else(|| self.subscriptions.first())
    }
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

impl Subscription {
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self.status, SubscriptionStatus::SubscriptionStatusActive)
    }

    #[must_use]
    pub fn active_until(&self) -> Option<&str> {
        self.stripe
            .as_ref()
            .and_then(|value| value.get("currentPeriodEnd"))
            .and_then(|value| value.as_str())
    }
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Subscription, SubscriptionStatus, SubscriptionsResponse, Tier};
    use crate::models::UserId;

    fn subscription(status: SubscriptionStatus, user_id: &str) -> Subscription {
        Subscription {
            xai_user_id: UserId::new(user_id),
            tier: Tier::SubscriptionTierGrokPro,
            status,
            create_time: "2026-01-01T00:00:00Z".to_owned(),
            mod_time: "2026-01-01T00:00:00Z".to_owned(),
            stripe: Some(json!({ "currentPeriodEnd": "2026-12-31T00:00:00Z" })),
            x_twitter: None,
        }
    }

    #[test]
    fn primary_prefers_active_subscription() {
        let inactive = subscription(
            SubscriptionStatus::SubscriptionStatusInactive,
            "inactive-user",
        );
        let active = subscription(SubscriptionStatus::SubscriptionStatusActive, "active-user");
        let response = SubscriptionsResponse {
            subscriptions: vec![inactive, active],
        };

        let primary = response.primary().expect("primary subscription");

        assert_eq!(primary.xai_user_id.as_str(), "active-user");
    }

    #[test]
    fn primary_falls_back_to_first_subscription() {
        let response = SubscriptionsResponse {
            subscriptions: vec![subscription(
                SubscriptionStatus::SubscriptionStatusInactive,
                "inactive-user",
            )],
        };

        let primary = response.primary().expect("primary subscription");

        assert_eq!(primary.xai_user_id.as_str(), "inactive-user");
    }
}
