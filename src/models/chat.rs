//! Chat request bodies, streaming event enum, and collected final-result shape.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::models::common::{
    AgentMessage, Citation, ConversationId, DeviceEnvInfo, FileMetadataId, FollowUpSuggestion,
    MessageTag, Mode, ModelResponse, ResponseId, ResponseStep, ToolOverrides, ToolUsageCard,
    ToolUsageCardId, UserResponse, WebSearchResult,
};

/// One hydrated response returned by `load_responses`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LoadedResponse {
    #[serde(rename = "responseId")]
    pub response_id: ResponseId,
    #[serde(default)]
    pub steps: Vec<ResponseStep>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `POST /rest/app-chat/conversations/<id>/load-responses` response body.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct LoadResponsesResponse {
    pub responses: Vec<LoadedResponse>,
}

/// Flags shared between `/conversations/new` and `/conversations/<id>/responses`.
///
/// Field names match the grok.com web-client wire format exactly. Boolean
/// defaults mirror an interactive browser session so omitting them at the call
/// site produces the same request shape a real browser would send.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChatRequestOptions {
    #[serde(rename = "disableSearch")]
    pub disable_search: bool,
    #[serde(rename = "enableImageGeneration")]
    pub enable_image_generation: bool,
    #[serde(rename = "returnImageBytes")]
    pub return_image_bytes: bool,
    #[serde(rename = "returnRawGrokInXaiRequest")]
    pub return_raw_grok_in_xai_request: bool,
    #[serde(rename = "enableImageStreaming")]
    pub enable_image_streaming: bool,
    #[serde(rename = "imageGenerationCount")]
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub image_generation_count: u32,
    #[serde(rename = "forceConcise")]
    pub force_concise: bool,
    #[serde(rename = "toolOverrides")]
    pub tool_overrides: ToolOverrides,
    #[serde(rename = "enableSideBySide")]
    pub enable_side_by_side: bool,
    #[serde(rename = "sendFinalMetadata")]
    pub send_final_metadata: bool,
    #[serde(rename = "disableTextFollowUps")]
    pub disable_text_follow_ups: bool,
    #[serde(rename = "responseMetadata")]
    pub response_metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "disableMemory")]
    pub disable_memory: bool,
    #[serde(rename = "forceSideBySide")]
    pub force_side_by_side: bool,
    #[serde(rename = "isAsyncChat")]
    pub is_async_chat: bool,
    #[serde(rename = "disableSelfHarmShortCircuit")]
    pub disable_self_harm_short_circuit: bool,
    #[serde(rename = "collectionIds")]
    pub collection_ids: Vec<String>,
    #[serde(rename = "connectors")]
    pub connectors: Vec<serde_json::Value>,
    #[serde(rename = "deviceEnvInfo")]
    pub device_env_info: DeviceEnvInfo,
}

impl Default for ChatRequestOptions {
    fn default() -> Self {
        Self {
            disable_search: false,
            enable_image_generation: false,
            return_image_bytes: false,
            return_raw_grok_in_xai_request: false,
            enable_image_streaming: true,
            image_generation_count: 0,
            force_concise: false,
            tool_overrides: ToolOverrides::default(),
            enable_side_by_side: false,
            send_final_metadata: true,
            disable_text_follow_ups: false,
            response_metadata: serde_json::Map::new(),
            disable_memory: false,
            force_side_by_side: false,
            is_async_chat: false,
            disable_self_harm_short_circuit: false,
            collection_ids: Vec::new(),
            connectors: Vec::new(),
            device_env_info: DeviceEnvInfo::default(),
        }
    }
}

/// Body for `POST /rest/app-chat/conversations/new`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct NewConversationRequest {
    pub temporary: bool,
    pub message: String,
    #[serde(rename = "fileAttachments")]
    pub file_attachments: Vec<FileMetadataId>,
    #[serde(rename = "imageAttachments")]
    pub image_attachments: Vec<FileMetadataId>,
    #[serde(rename = "modeId")]
    pub mode_id: Mode,
    #[serde(flatten)]
    pub options: ChatRequestOptions,
}

/// Body for `POST /rest/app-chat/conversations/<id>/responses`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ContinueConversationRequest {
    pub message: String,
    #[serde(rename = "parentResponseId", skip_serializing_if = "Option::is_none")]
    pub parent_response_id: Option<ResponseId>,
    #[serde(rename = "fileAttachments")]
    pub file_attachments: Vec<FileMetadataId>,
    #[serde(rename = "imageAttachments")]
    pub image_attachments: Vec<FileMetadataId>,
    /// `modeId` is optional on continuations — if omitted, grok.com reuses the
    /// conversation's current mode.
    #[serde(rename = "modeId", skip_serializing_if = "Option::is_none")]
    pub mode_id: Option<Mode>,
    #[serde(flatten)]
    pub options: ChatRequestOptions,
}

/// Normalized semantic event from the NDJSON stream.
///
/// Both observed envelope shapes (`result.response.foo` and `result.foo`) are
/// flattened into this single enum by [`crate::client::stream`]. Unknown event
/// kinds land in [`StreamEvent::Unknown`] instead of being silently dropped.
#[derive(Clone, Debug)]
pub enum StreamEvent {
    /// `result.conversation` — only emitted by `/conversations/new`.
    Conversation(Box<crate::models::common::Conversation>),
    /// `result.response.userResponse` / `result.userResponse` — echoed user turn.
    UserResponse(Box<UserResponse>),
    /// `result.(response.)uiLayout` — layout hint for the client UI.
    UiLayout {
        response_id: Option<ResponseId>,
        layout: serde_json::Value,
    },
    /// `result.(response.)llmInfo` — opaque model hash.
    LlmInfo {
        response_id: Option<ResponseId>,
        model_hash: String,
    },
    /// `result.progressReport` — multi-stage progress update.
    ProgressReport {
        response_id: Option<ResponseId>,
        category: String,
        state: String,
        message: Option<String>,
    },
    /// Streamed text fragment; the `token` payload is raw string.
    Token {
        response_id: Option<ResponseId>,
        token: String,
        message_tag: Option<MessageTag>,
        message_step_id: Option<u32>,
        is_thinking: bool,
        is_soft_stop: bool,
        tool_usage_card_id: Option<ToolUsageCardId>,
    },
    /// `result.(response.)toolUsageCard` — structured tool invocation card.
    ToolUsageCard(Box<ToolUsageCard>),
    /// `result.(response.)webSearchResults` — one batch of search hits.
    WebSearchResults {
        response_id: Option<ResponseId>,
        tool_usage_card_id: Option<ToolUsageCardId>,
        message_tag: Option<MessageTag>,
        results: Vec<WebSearchResult>,
    },
    /// `result.(response.)finalMetadata` — follow-up suggestions and feedback hints.
    FinalMetadata {
        response_id: Option<ResponseId>,
        follow_up_suggestions: Vec<FollowUpSuggestion>,
        disclaimer: Option<String>,
    },
    /// `result.(response.)modelResponse` — terminal assembled assistant message.
    ModelResponse(Box<ModelResponse>),
    /// `result.title` — title suggestion after `/conversations/new`.
    Title { new_title: String },
    /// Any frame we couldn't classify; preserved verbatim for debugging.
    Unknown(serde_json::Value),
}

/// Drained result of a streaming chat request, suitable for a single tool reply.
///
/// `steps` is populated only when the caller asks for hydrated output via
/// `verbosity = "full"` (or the deprecated `full_details = true` alias).
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct FinalChatResult {
    pub conversation_id: ConversationId,
    pub response_id: ResponseId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_response_id: Option<ResponseId>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Inline citations extracted from `<grok:render>` message markup and the
    /// corresponding `cardAttachment.jsonData` stream frames.
    pub citations: Vec<Citation>,
    pub tool_usage_cards: Vec<ToolUsageCard>,
    pub web_search_results: Vec<WebSearchResult>,
    pub follow_up_suggestions: Vec<FollowUpSuggestion>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<Vec<ResponseStep>>,
    /// Multi-agent chatroom messages extracted from hydrated `steps` when the
    /// caller requests `verbosity = "full"`.
    pub agent_messages: Vec<AgentMessage>,
    /// Unclassified stream frames (objects only); empty in the happy path.
    ///
    /// The `StreamEvent::Unknown` variant itself can carry any JSON value, but
    /// only object-shaped frames survive into the collected result. Non-object
    /// payloads get wrapped as `{"__non_object": <value>}` before landing here
    /// so every element serialises to a concrete JSON Schema object — strict
    /// Zod-based MCP clients reject the `{ "type": true }` that schemars 1.x
    /// emits for bare [`serde_json::Value`].
    pub unknown_events: Vec<serde_json::Map<String, serde_json::Value>>,
}

#[cfg(test)]
mod tests {
    use super::LoadResponsesResponse;

    #[test]
    fn load_responses_roundtrip_keeps_typed_steps() {
        let raw = r#"{
          "responses": [
            {
              "responseId": "response-fabricated-1",
              "steps": [
                {
                  "text": ["searched and summarized"],
                  "tags": ["analysis", "search"],
                  "rolloutId": "rollout-fabricated-1",
                  "messageStepId": 7,
                  "webSearchResults": [
                    {
                      "url": "https://example.com/article",
                      "title": "Example Result",
                      "preview": "example preview"
                    }
                  ],
                  "toolUsageCards": [
                    {
                      "toolUsageCardId": "tool-card-fabricated-1",
                      "kind": "web_search"
                    }
                  ],
                  "toolUsageResults": [
                    {
                      "resultType": "web_search",
                      "status": "ok"
                    }
                  ],
                  "unexpectedField": true
                }
              ],
              "sender": "ASSISTANT"
            }
          ]
        }"#;

        let parsed = serde_json::from_str::<LoadResponsesResponse>(raw).expect("parse");
        let response = parsed.responses.first().expect("response");
        let step = response.steps.first().expect("step");

        assert_eq!(response.response_id.as_str(), "response-fabricated-1");
        assert_eq!(step.text, vec!["searched and summarized"]);
        assert_eq!(step.tags, vec!["analysis", "search"]);
        assert_eq!(
            step.rollout_id.as_ref().expect("rollout").as_str(),
            "rollout-fabricated-1"
        );
        assert_eq!(step.message_step_id, Some(7));
        assert_eq!(step.web_search_results.len(), 1);
        assert_eq!(step.tool_usage_cards.len(), 1);
        assert_eq!(step.tool_usage_results.len(), 1);
        assert_eq!(
            step.extra
                .get("unexpectedField")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            response
                .extra
                .get("sender")
                .and_then(|value| value.as_str()),
            Some("ASSISTANT")
        );

        let reparsed = serde_json::from_str::<LoadResponsesResponse>(
            &serde_json::to_string(&parsed).expect("serialize"),
        )
        .expect("reparse");
        let reparsed_step = reparsed
            .responses
            .first()
            .expect("response")
            .steps
            .first()
            .expect("step");
        assert_eq!(reparsed_step.text, vec!["searched and summarized"]);
        assert_eq!(reparsed_step.tool_usage_results.len(), 1);
    }
}
