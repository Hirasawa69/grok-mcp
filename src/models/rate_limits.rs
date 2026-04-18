//! `POST /rest/rate-limits` payloads.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Response shape observed from grok.com.
///
/// `low_effort_rate_limits` and `high_effort_rate_limits` are nullable on the
/// wire; they are populated only for modes that distinguish reasoning-effort
/// tiers.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RateLimits {
    #[serde(rename = "windowSizeSeconds")]
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub window_size_seconds: u64,
    #[serde(rename = "remainingQueries")]
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub remaining_queries: u64,
    #[serde(rename = "totalQueries")]
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub total_queries: u64,
    #[serde(
        default,
        rename = "lowEffortRateLimits",
        skip_serializing_if = "Option::is_none"
    )]
    pub low_effort_rate_limits: Option<RateLimitTier>,
    #[serde(
        default,
        rename = "highEffortRateLimits",
        skip_serializing_if = "Option::is_none"
    )]
    pub high_effort_rate_limits: Option<RateLimitTier>,
}

/// Sub-limit for a specific effort tier (low/high). Schema is open because
/// observed responses carry `null` for both; real field names are inferred
/// from the surrounding container only.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct RateLimitTier {
    #[serde(
        default,
        rename = "windowSizeSeconds",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "crate::server::schema_helpers::option_u64_schema")]
    pub window_size_seconds: Option<u64>,
    #[serde(
        default,
        rename = "remainingQueries",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "crate::server::schema_helpers::option_u64_schema")]
    pub remaining_queries: Option<u64>,
    #[serde(
        default,
        rename = "totalQueries",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "crate::server::schema_helpers::option_u64_schema")]
    pub total_queries: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
