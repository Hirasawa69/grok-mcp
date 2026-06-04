//! Output structs for each MCP tool.

use schemars::JsonSchema;
use serde::Serialize;

use crate::{
    config::ChatDefaults,
    models::{
        chat::FinalChatResult,
        common::{Conversation, ResponseId, ResponseNode, RolloutId, UserId},
        rate_limits::{RateLimitTier, RateLimits},
        subscriptions::{SubscriptionStatus, Tier},
    },
};

#[derive(Debug, Serialize, JsonSchema)]
pub struct AuthStatus {
    pub authenticated: bool,
    pub user_id: Option<UserId>,
    pub tier: Option<Tier>,
    pub status: Option<SubscriptionStatus>,
    pub active_until: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<AuthSubscription>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct AuthSubscription {
    pub user_id: UserId,
    pub tier: Tier,
    pub status: SubscriptionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_until: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct RateLimitsOutput {
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub window_seconds: u64,
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub remaining: u64,
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub low_effort: Option<RateLimitTier>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub high_effort: Option<RateLimitTier>,
}

impl From<RateLimits> for RateLimitsOutput {
    fn from(value: RateLimits) -> Self {
        Self {
            window_seconds: value.window_size_seconds,
            remaining: value.remaining_queries,
            total: value.total_queries,
            low_effort: value.low_effort_rate_limits,
            high_effort: value.high_effort_rate_limits,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ListConversationsOutput {
    pub conversations: Vec<Conversation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct GetConversationOutput {
    #[schemars(schema_with = "crate::server::schema_helpers::conversation_output_schema")]
    pub conversation: Conversation,
    #[schemars(schema_with = "crate::server::schema_helpers::option_response_nodes_schema")]
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_nodes: Option<Vec<ResponseNode>>,
    #[schemars(schema_with = "crate::server::schema_helpers::option_loaded_responses_schema")]
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responses: Option<Vec<serde_json::Value>>,
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub message_count: u32,
    #[schemars(schema_with = "crate::server::schema_helpers::u64_schema")]
    pub total_chars: u64,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ResearchStartOutput {
    pub conversation_id: crate::models::common::ConversationId,
    pub response_id: ResponseId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_response_id: Option<ResponseId>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct StepThinking {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollout_id: Option<RolloutId>,
    pub tags: Vec<String>,
    pub text: String,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PollStatus {
    /// Grok is still generating the response.
    InProgress,
    /// Response complete; [`PollOutput::result`] is populated.
    Completed,
    /// No response matches the given `(conversation_id, response_id)` pair.
    NotFound,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PollOutput {
    /// Current state of the requested research result.
    pub status: PollStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Box<FinalChatResult>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_steps: Option<Vec<StepThinking>>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct UploadFileOutput {
    pub file_metadata_id: crate::models::common::FileMetadataId,
    pub file_uri: String,
    pub mime_type: String,
    pub file_name: String,
    pub create_time: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct DefaultsOutput {
    pub defaults: ChatDefaults,
}
