//! Domain types for the grok.com REST + NDJSON surface.
//!
//! All payloads live under this module as plain `serde` structs; nothing here
//! touches HTTP, cookies, or MCP. The split mirrors the grok.com endpoint families:
//!
//! * [`common`] — shared newtypes ([`common::ConversationId`], [`common::ResponseId`], …)
//!   plus the `Conversation`, `ResponseNode`, `Mode`, `MessageTag` primitives.
//! * [`chat`] — request bodies for `/conversations/new` and `/conversations/<id>/responses`,
//!   the normalized `StreamEvent` enum, and the collected `FinalChatResult`.
//! * [`rate_limits`], [`subscriptions`], [`skills`] — small REST payloads.
//! * [`upload`] — `/rest/app-chat/upload-file` request/response + asset metadata.
//! * [`api_error`] — the `{code, message, details}` envelope used on non-2xx responses.

pub mod api_error;
pub mod chat;
pub mod common;
pub mod rate_limits;
pub mod skills;
pub mod subscriptions;
pub mod upload;

pub use api_error::GrokApiError;
pub use chat::{
    ChatRequestOptions, ContinueConversationRequest, FinalChatResult, LoadResponsesResponse,
    LoadedResponse, NewConversationRequest, StreamEvent,
};
pub use common::{
    AgentMessage, Citation, Conversation, ConversationId, DeviceEnvInfo, FileMetadataId,
    FollowUpSuggestion, MessageTag, Mode, ModelResponse, ResponseId, ResponseNode, ResponseStep,
    RolloutId, Sender, ToolOverrides, ToolUsageCard, ToolUsageCardId, UserId, UserResponse,
    WebSearchResult,
};
pub use rate_limits::{RateLimitTier, RateLimits};
pub use skills::{Skill, SkillsResponse};
pub use subscriptions::{Subscription, SubscriptionStatus, SubscriptionsResponse, Tier};
pub use upload::{AssetMetadata, UploadRequest, UploadResponse};
