//! Shared newtypes and enums used across the grok.com domain.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wrap a raw string as this newtype without validation.
            ///
            /// grok.com returns these as opaque UUID-ish values; the server is
            /// the source of truth, so no shape is enforced here.
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, formatter)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

string_newtype!(
    /// Identifier of a grok.com conversation (server-assigned UUID).
    ConversationId
);
string_newtype!(
    /// Identifier of a single response node (assistant or user turn).
    ResponseId
);
string_newtype!(
    /// Identifier for a tool-usage card emitted inside a streaming response.
    ToolUsageCardId
);
string_newtype!(
    /// Identifier returned by `/rest/app-chat/upload-file` — later attached as
    /// `fileAttachments[…]` on a chat request.
    FileMetadataId
);
string_newtype!(
    /// Grok/xAI user id embedded in `/rest/subscriptions` and auth cookies.
    UserId
);
string_newtype!(
    /// Client-generated `x-xai-request-id` header value (fresh per request).
    RequestId
);
string_newtype!(
    /// Identifier for the backend rollout that produced a hydrated response step.
    RolloutId
);

/// Inline citation extracted from `message` text and `cardAttachment` stream
/// frames. The `card_id` links the citation marker in the message to the
/// source URL.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Citation {
    /// Matches the `card_id` attribute in the `<grok:render>` tag and
    /// `jsonData.id` in the matching `cardAttachment` stream frame.
    pub card_id: String,
    /// The `citation_id` from the inner `<argument>` element.
    ///
    /// This does not index into `web_search_results`; treat it as an opaque
    /// Grok-internal reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub citation_id: Option<String>,
    /// Always `citation_card` in observed traffic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_type: Option<String>,
    /// Source URL for this citation.
    ///
    /// This is the only authoritative URL; `web_search_results[citation_id]`
    /// is not reliable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// A message from one agent to another in a multi-agent conversation.
/// Extracted from `chatroomSend` tool cards in hydrated steps.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct AgentMessage {
    /// Who sent the message — the `rolloutId` of the step that contained the
    /// `chatroomSend` card.
    pub from: RolloutId,
    /// Who received the message.
    pub to: String,
    /// The agent's message text.
    pub text: String,
}

/// Author of a message in a conversation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum Sender {
    /// Message authored by the end-user (local agent prompt).
    Human,
    /// Message authored by the assistant (model output).
    Assistant,
    /// Rare: system control / tool-delivered content.
    #[serde(other)]
    Other,
}

/// Grok `modeId` / `modelName` value on chat requests.
///
/// The enum is non-exhaustive at the wire layer: only `auto` and `expert` are
/// confirmed from observed grok.com web traffic. Other values (`fast`,
/// `grok-3`, …) have been rumoured historically. Callers can pass arbitrary
/// strings via [`Mode::Other`] without waiting for an enum bump.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `"auto"` — grok.com router picks the backing model.
    #[default]
    Auto,
    /// `"expert"` — long-reasoning mode.
    Expert,
    /// `"fast"` — rumoured lightweight mode; not independently confirmed.
    Fast,
    /// Any other string; preserved verbatim on the wire.
    #[serde(untagged)]
    Other(String),
}

impl Mode {
    /// Return the wire string (`"auto"`, `"expert"`, the `Other` payload, …).
    #[must_use]
    pub fn as_wire(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Expert => "expert",
            Self::Fast => "fast",
            Self::Other(raw) => raw.as_str(),
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_wire())
    }
}

/// Token sub-type reported on each `token` stream frame.
///
/// Confirmed tags from observed grok.com traffic: `header`, `summary`,
/// `final`, `raw_function_result`, `tool_usage_card`. Any other string is
/// preserved verbatim via [`MessageTag::Other`].
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageTag {
    /// Header/title fragment of the thinking trace.
    Header,
    /// Summary fragment of the thinking trace.
    Summary,
    /// Final visible answer token.
    Final,
    /// Raw result body produced by an inline tool call.
    RawFunctionResult,
    /// Tool-usage card payload fragment.
    ToolUsageCard,
    /// Any unknown tag; preserved verbatim.
    #[serde(untagged)]
    Other(String),
}

/// Per-conversation tool-override flags emitted on chat requests.
///
/// The grok.com web client always sends these five flags; we model them as
/// `Option<bool>` so only the explicitly set ones go on the wire. The server
/// tolerates missing fields, so we don't force any defaults here.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ToolOverrides {
    #[serde(rename = "gmailSearch", skip_serializing_if = "Option::is_none")]
    pub gmail_search: Option<bool>,
    #[serde(
        rename = "googleCalendarSearch",
        skip_serializing_if = "Option::is_none"
    )]
    pub google_calendar_search: Option<bool>,
    #[serde(rename = "outlookSearch", skip_serializing_if = "Option::is_none")]
    pub outlook_search: Option<bool>,
    #[serde(
        rename = "outlookCalendarSearch",
        skip_serializing_if = "Option::is_none"
    )]
    pub outlook_calendar_search: Option<bool>,
    #[serde(rename = "googleDriveSearch", skip_serializing_if = "Option::is_none")]
    pub google_drive_search: Option<bool>,
}

impl ToolOverrides {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.gmail_search.is_none()
            && self.google_calendar_search.is_none()
            && self.outlook_search.is_none()
            && self.outlook_calendar_search.is_none()
            && self.google_drive_search.is_none()
    }
}

/// Browser viewport metadata sent on every chat request.
///
/// Defaults approximate a desktop browser window.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceEnvInfo {
    #[serde(rename = "darkModeEnabled")]
    pub dark_mode_enabled: bool,
    #[serde(rename = "devicePixelRatio")]
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub device_pixel_ratio: u32,
    #[serde(rename = "screenWidth")]
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub screen_width: u32,
    #[serde(rename = "screenHeight")]
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub screen_height: u32,
    #[serde(rename = "viewportWidth")]
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub viewport_width: u32,
    #[serde(rename = "viewportHeight")]
    #[schemars(schema_with = "crate::server::schema_helpers::u32_schema")]
    pub viewport_height: u32,
}

impl Default for DeviceEnvInfo {
    fn default() -> Self {
        Self {
            dark_mode_enabled: false,
            device_pixel_ratio: 1,
            screen_width: 1366,
            screen_height: 768,
            viewport_width: 1366,
            viewport_height: 682,
        }
    }
}

/// Conversation list item / detail record.
///
/// Extra fields from grok.com (`workspaces`, `taskResult`, …) are preserved on
/// the wire but not exposed as typed accessors for v0.1.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Conversation {
    #[serde(rename = "conversationId")]
    pub conversation_id: ConversationId,
    pub title: String,
    #[serde(default)]
    pub starred: bool,
    #[serde(rename = "createTime")]
    pub create_time: String,
    #[serde(rename = "modifyTime")]
    pub modify_time: String,
    #[serde(default, rename = "systemPromptName")]
    pub system_prompt_name: String,
    #[serde(default)]
    pub temporary: bool,
    /// Catch-all for fields we don't model yet.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// `response-node?includeThreads=true` list item.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ResponseNode {
    #[serde(rename = "responseId")]
    pub response_id: ResponseId,
    pub sender: Sender,
    #[serde(
        default,
        rename = "parentResponseId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_response_id: Option<ResponseId>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// One step inside a hydrated assistant response.
///
/// Populated by `load_responses`; empty on a bare stream result. `text` is a
/// list of fragments because the wire format batches per-step content as an
/// array, even when a step carries only one string.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ResponseStep {
    pub text: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, rename = "rolloutId", skip_serializing_if = "Option::is_none")]
    pub rollout_id: Option<RolloutId>,
    #[serde(
        default,
        rename = "messageStepId",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(schema_with = "crate::server::schema_helpers::option_u32_schema")]
    pub message_step_id: Option<u32>,
    #[serde(default, rename = "webSearchResults")]
    pub web_search_results: Vec<WebSearchResult>,
    #[serde(default, rename = "toolUsageCards")]
    pub tool_usage_cards: Vec<ToolUsageCard>,
    /// Raw `toolUsageResults` entries; per-tool shape varies so we keep them as
    /// objects rather than forcing a typed variant.
    #[serde(default, rename = "toolUsageResults")]
    pub tool_usage_results: Vec<serde_json::Map<String, Value>>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Echoed user turn emitted on the stream before the assistant begins writing.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct UserResponse {
    #[serde(rename = "responseId")]
    pub response_id: ResponseId,
    pub message: String,
    #[serde(default)]
    pub sender: Option<Sender>,
    #[serde(
        default,
        rename = "parentResponseId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_response_id: Option<ResponseId>,
    #[serde(default, rename = "createTime")]
    pub create_time: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Terminal stream event carrying the fully assembled assistant message.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ModelResponse {
    #[serde(rename = "responseId")]
    pub response_id: ResponseId,
    pub message: String,
    #[serde(default)]
    pub sender: Option<Sender>,
    #[serde(
        default,
        rename = "parentResponseId",
        skip_serializing_if = "Option::is_none"
    )]
    pub parent_response_id: Option<ResponseId>,
    #[serde(default, rename = "createTime")]
    pub create_time: Option<String>,
    #[serde(
        default,
        rename = "thinkingStartTime",
        skip_serializing_if = "Option::is_none"
    )]
    pub thinking_start_time: Option<String>,
    #[serde(
        default,
        rename = "thinkingEndTime",
        skip_serializing_if = "Option::is_none"
    )]
    pub thinking_end_time: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// One search hit inside a `webSearchResults` stream frame.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchResult {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub preview: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// Tool-card structured payload (per-tool shape kept as raw JSON).
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ToolUsageCard {
    #[serde(rename = "toolUsageCardId")]
    pub tool_usage_card_id: ToolUsageCardId,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

/// `finalMetadata.followUpSuggestions[*]` entry.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct FollowUpSuggestion {
    pub label: String,
    #[serde(default)]
    pub properties: serde_json::Map<String, Value>,
    #[serde(default, rename = "toolOverrides")]
    pub tool_overrides: Option<ToolOverrides>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_roundtrip() {
        let raw = r#"{"conversationId":"42a550c9-e919-b625-z615-6590a44e3e70","title":"New conversation","starred":false,"createTime":"2026-04-17T18:56:12.214583Z","modifyTime":"2026-04-17T18:56:12.215104Z","systemPromptName":"","temporary":false,"mediaTypes":[],"workspaces":[],"taskResult":null}"#;
        let conv = serde_json::from_str::<Conversation>(raw).expect("parse");
        assert_eq!(conv.title, "New conversation");
        assert_eq!(
            conv.conversation_id.as_str(),
            "42a550c9-e919-b625-z615-6590a44e3e70"
        );
        assert!(!conv.starred);
        assert!(conv.extra.contains_key("mediaTypes"));
    }

    #[test]
    fn mode_wire_format() {
        let wire = serde_json::to_string(&Mode::Expert).expect("serialize");
        assert_eq!(wire, "\"expert\"");
        let other = serde_json::to_string(&Mode::Other("fast".to_owned())).expect("serialize");
        assert_eq!(other, "\"fast\"");
    }

    #[test]
    fn message_tag_unknown_preserved() {
        let tag = serde_json::from_str::<MessageTag>("\"brand_new_tag\"").expect("parse");
        assert!(matches!(tag, MessageTag::Other(_)));
    }

    #[test]
    fn response_step_text_deserializes_as_fragments() {
        let single =
            serde_json::from_str::<ResponseStep>(r#"{"text":["hello"],"tags":["header"]}"#)
                .expect("single fragment step");
        assert_eq!(single.text, vec!["hello".to_owned()]);
        assert_eq!(single.tags, vec!["header"]);

        let multiple = serde_json::from_str::<ResponseStep>(
            r#"{"text":["frag one","frag two"],"tags":["final"]}"#,
        )
        .expect("multi fragment step");
        assert_eq!(multiple.text.len(), 2);
        assert_eq!(
            multiple.text,
            vec!["frag one".to_owned(), "frag two".to_owned()]
        );
        assert_eq!(multiple.tags, vec!["final"]);
    }
}
