//! User-facing chat-request options.
//!
//! These mirror the subset of grok.com's chat-request wire format that a
//! caller is likely to override per-call. Everything here is optional; the
//! client fills unset fields from the current runtime [`crate::config::ChatDefaults`]
//! at send time. Keep this module lean — new fields go in only when a concrete
//! caller needs them.

use bon::Builder;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    config::ChatDefaults,
    models::{
        ChatRequestOptions, ContinueConversationRequest, FileMetadataId, Mode,
        NewConversationRequest, ResponseId, ToolOverrides,
    },
};

/// Per-call chat-request overrides.
///
/// Every field is optional. The client merges these over the active
/// [`ChatDefaults`] when building the wire-format request, so omitting a field
/// means "use the current default". Build one via [`ChatOptions::builder`] or
/// construct it with a struct literal plus `..Default::default()`.
#[derive(Builder, Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ChatOptions {
    /// Request mode override. `None` means inherit the runtime default for new
    /// conversations, or reuse the thread's current mode for continuations.
    pub mode: Option<Mode>,
    /// File attachments uploaded via `grok_upload_file`. Empty by default.
    #[builder(default)]
    pub attachments: Vec<FileMetadataId>,
    /// Suppress web search for this call.
    pub disable_search: Option<bool>,
    /// Force short-form answers.
    pub force_concise: Option<bool>,
    /// Suppress Grok's memory system for this call.
    pub disable_memory: Option<bool>,
    /// Per-integration toggles.
    #[builder(default)]
    pub integrations: IntegrationFlags,
}

/// Per-integration override toggles mirrored from the wire `toolOverrides`
/// object.
#[derive(Builder, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct IntegrationFlags {
    /// Override Gmail search availability for this call.
    pub gmail: Option<bool>,
    /// Override Google Calendar search availability for this call.
    pub google_calendar: Option<bool>,
    /// Override Outlook search availability for this call.
    pub outlook: Option<bool>,
    /// Override Outlook Calendar search availability for this call.
    pub outlook_calendar: Option<bool>,
    /// Override Google Drive search availability for this call.
    pub google_drive: Option<bool>,
}

impl ChatOptions {
    pub(crate) fn into_new_conversation_request(
        self,
        message: String,
        defaults: &ChatDefaults,
    ) -> NewConversationRequest {
        let ChatOptions {
            mode,
            attachments,
            disable_search,
            force_concise,
            disable_memory,
            integrations,
        } = self;

        NewConversationRequest {
            temporary: false,
            message,
            file_attachments: attachments,
            image_attachments: Vec::new(),
            mode_id: mode.clone().unwrap_or(defaults.mode.clone()),
            options: ChatOptions {
                mode,
                attachments: Vec::new(),
                disable_search,
                force_concise,
                disable_memory,
                integrations,
            }
            .into_wire(defaults),
        }
    }

    pub(crate) fn into_continue_conversation_request(
        self,
        message: Option<String>,
        parent_response_id: Option<ResponseId>,
        defaults: &ChatDefaults,
    ) -> ContinueConversationRequest {
        let ChatOptions {
            mode,
            attachments,
            disable_search,
            force_concise,
            disable_memory,
            integrations,
        } = self;

        ContinueConversationRequest {
            message: message.unwrap_or_default(),
            parent_response_id,
            file_attachments: attachments,
            image_attachments: Vec::new(),
            mode_id: mode,
            options: ChatOptions {
                mode: None,
                attachments: Vec::new(),
                disable_search,
                force_concise,
                disable_memory,
                integrations,
            }
            .into_wire(defaults),
        }
    }

    pub(crate) fn into_wire(self, defaults: &ChatDefaults) -> ChatRequestOptions {
        ChatRequestOptions {
            disable_search: self.disable_search.unwrap_or(defaults.disable_search),
            force_concise: self.force_concise.unwrap_or(defaults.force_concise),
            disable_memory: self.disable_memory.unwrap_or(defaults.disable_memory),
            tool_overrides: ToolOverrides {
                gmail_search: self.integrations.gmail,
                google_calendar_search: self.integrations.google_calendar,
                outlook_search: self.integrations.outlook,
                outlook_calendar_search: self.integrations.outlook_calendar,
                google_drive_search: self.integrations.google_drive,
            },
            enable_image_generation: defaults.enable_image_generation,
            image_generation_count: defaults.image_generation_count,
            enable_side_by_side: defaults.enable_side_by_side,
            disable_text_follow_ups: defaults.disable_text_follow_ups,
            ..ChatRequestOptions::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ChatOptions, IntegrationFlags};
    use crate::{
        config::ChatDefaults,
        models::{FileMetadataId, Mode, ResponseId},
    };

    #[test]
    fn new_conversation_request_inherits_default_mode_and_keeps_attachments() {
        let defaults = ChatDefaults::default();
        let attachment_id = FileMetadataId::new("file-1");
        let request = ChatOptions::builder()
            .attachments(vec![attachment_id.clone()])
            .disable_search(true)
            .force_concise(true)
            .disable_memory(true)
            .integrations(IntegrationFlags::builder().gmail(true).build())
            .build()
            .into_new_conversation_request("hello".to_owned(), &defaults);

        let value = serde_json::to_value(&request).expect("serialize new conversation request");

        assert_eq!(request.message, "hello");
        assert_eq!(request.mode_id, defaults.mode);
        assert_eq!(request.file_attachments, vec![attachment_id]);
        assert_eq!(request.image_attachments, Vec::<FileMetadataId>::new());
        assert_eq!(value.get("modeId"), Some(&json!("expert")));
        assert_eq!(value.get("fileAttachments"), Some(&json!(["file-1"])));
        assert_eq!(value.get("disableSearch"), Some(&json!(true)));
        assert_eq!(value.get("forceConcise"), Some(&json!(true)));
        assert_eq!(value.get("disableMemory"), Some(&json!(true)));
        assert_eq!(
            value
                .get("toolOverrides")
                .and_then(|tool_overrides| tool_overrides.get("gmailSearch")),
            Some(&json!(true))
        );
    }

    #[test]
    fn continue_conversation_request_uses_empty_message_without_override() {
        let defaults = ChatDefaults::default();
        let attachment_id = FileMetadataId::new("file-2");
        let parent_response_id = ResponseId::new("parent-1");
        let request = ChatOptions::builder()
            .attachments(vec![attachment_id.clone()])
            .disable_search(true)
            .mode(Mode::Fast)
            .integrations(IntegrationFlags::builder().outlook(false).build())
            .build()
            .into_continue_conversation_request(None, Some(parent_response_id.clone()), &defaults);

        let value = serde_json::to_value(&request).expect("serialize continuation request");

        assert_eq!(request.message, "");
        assert_eq!(request.parent_response_id, Some(parent_response_id));
        assert_eq!(request.mode_id, Some(Mode::Fast));
        assert_eq!(request.file_attachments, vec![attachment_id]);
        assert_eq!(value.get("message"), Some(&json!("")));
        assert_eq!(value.get("modeId"), Some(&json!("fast")));
        assert_eq!(value.get("parentResponseId"), Some(&json!("parent-1")));
        assert_eq!(
            value
                .get("toolOverrides")
                .and_then(|tool_overrides| tool_overrides.get("outlookSearch")),
            Some(&json!(false))
        );
        assert_eq!(value.get("disableSearch"), Some(&json!(true)));
    }
}
