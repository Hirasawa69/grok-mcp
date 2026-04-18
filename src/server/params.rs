//! Input structs for each MCP tool. Every struct derives `Deserialize` and
//! `JsonSchema` so rmcp can generate a tool input schema automatically.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::{ConversationId, FileMetadataId, Mode, ResponseId};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct RateLimitsParams {
    /// Mode to query rate limits for. Defaults to the configured runtime mode.
    #[serde(default)]
    pub mode: Option<Mode>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct AskParams {
    /// Prompt text for Grok.
    pub message: String,
    /// Existing conversation id to continue. If absent, a new conversation is
    /// created with `POST /conversations/new`.
    #[serde(default)]
    pub conversation_id: Option<ConversationId>,
    /// Parent response id for branching / continuation (matches grok.com web UI).
    #[serde(default)]
    pub parent_response_id: Option<ResponseId>,
    /// Override the runtime-default mode for this call only.
    #[serde(default)]
    pub mode: Option<Mode>,
    /// Override the runtime-default "disable web search" flag for this call only.
    #[serde(default)]
    pub disable_search: Option<bool>,
    /// Override the runtime-default "force concise" flag for this call only.
    #[serde(default)]
    pub force_concise: Option<bool>,
    /// Override the runtime-default "disable memory" flag for this call only.
    #[serde(default)]
    pub disable_memory: Option<bool>,
    /// Enable Gmail search for this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_gmail_search: Option<bool>,
    /// Enable Google Calendar search for this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_google_calendar_search: Option<bool>,
    /// Enable Outlook search for this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_outlook_search: Option<bool>,
    /// Enable Outlook Calendar search for this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_outlook_calendar_search: Option<bool>,
    /// Enable Google Drive search for this call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_google_drive_search: Option<bool>,
    /// `fileMetadataId` values returned by `grok_upload_file`; attached as
    /// `fileAttachments` on the outgoing chat request.
    #[serde(default)]
    pub attachments: Vec<FileMetadataId>,
    /// When true, hydrate the response with `steps[]` (per-step tags,
    /// rolloutId, toolUsageCards, toolUsageResults, webSearchResults) by
    /// calling response-node + load-responses after the stream ends.
    /// Adds one extra HTTP round-trip.
    #[serde(default)]
    pub full_details: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct PollParams {
    pub conversation_id: String,
    pub response_id: String,
    /// When true and the response is ready, hydrate `steps[]` and
    /// `agent_messages[]` the same way as `grok_research(full_details=true)`.
    #[serde(default)]
    pub full_details: bool,
    /// Include per-step thinking traces (agent reasoning, tool calls). Off by
    /// default to save context window. Each entry shows one step with its
    /// `rolloutId` and raw text.
    #[serde(default)]
    pub include_thinking: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListConversationsParams {
    /// Page size; defaults to 60 to match the web client.
    #[serde(default)]
    #[schemars(schema_with = "crate::server::schema_helpers::option_u32_schema")]
    pub page_size: Option<u32>,
    /// Cursor returned by a prior call.
    #[serde(default)]
    pub page_token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct GetConversationParams {
    pub conversation_id: ConversationId,
    /// Hydrate the conversation with its response thread as well.
    #[serde(default)]
    pub include_messages: bool,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct UploadFileParams {
    pub file_name: String,
    /// Content MIME type. Falls back to `application/octet-stream` if omitted.
    #[serde(default)]
    pub mime_type: Option<String>,
    /// Base64-encoded file bytes. Exactly one of `content_base64` / `local_path`
    /// must be set.
    #[serde(default)]
    pub content_base64: Option<String>,
    /// Local filesystem path to read and upload. Exactly one of
    /// `content_base64` / `local_path` must be set.
    #[serde(default)]
    pub local_path: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct SkillsParams {
    /// BCP-47 locale hint, e.g. `"en"`.
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct SetDefaultsParams {
    pub mode: Option<Mode>,
    pub disable_search: Option<bool>,
    pub force_concise: Option<bool>,
    pub disable_memory: Option<bool>,
    pub enable_image_generation: Option<bool>,
    #[schemars(schema_with = "crate::server::schema_helpers::option_u32_schema")]
    pub image_generation_count: Option<u32>,
    pub enable_side_by_side: Option<bool>,
    pub disable_text_follow_ups: Option<bool>,
}
