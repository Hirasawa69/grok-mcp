//! Conversation-oriented client API: list, get, start, continue, and hydrate.

use bon::Builder;
use serde::Deserialize;
use serde_json::json;

use crate::{
    client::{GrokClient, stream::StreamHandle},
    config::ChatDefaults,
    error::{ConfigError, Error, Result},
    models::{
        ChatRequestOptions, ContinueConversationRequest, Conversation, ConversationId,
        FileMetadataId, LoadResponsesResponse, LoadedResponse, Mode, NewConversationRequest,
        ResponseId, ResponseNode, ToolOverrides,
    },
    server::output::GetConversationOutput,
};

/// List wrapper for `GET /rest/app-chat/conversations`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ConversationList {
    pub conversations: Vec<Conversation>,
    #[serde(
        default,
        rename = "nextPageToken",
        skip_serializing_if = "Option::is_none"
    )]
    pub next_page_token: Option<String>,
}

/// `GET /rest/app-chat/conversations/<id>/response-node?includeThreads=true` response.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ResponseNodeResult {
    #[serde(rename = "responseNodes")]
    pub response_nodes: Vec<ResponseNode>,
}

#[derive(Deserialize)]
struct ConversationEnvelope {
    conversation: Conversation,
}

/// Resource entry point for conversation-oriented API calls.
#[derive(Clone, Copy)]
pub struct Conversations<'a>(&'a GrokClient);

/// Data assembled by the generated `StartConversationBuilder`.
#[derive(Builder)]
pub struct StartConversation<'a> {
    #[builder(start_fn)]
    client: &'a GrokClient,
    #[builder(into)]
    message: Option<String>,
    mode: Option<Mode>,
    #[builder(default)]
    attachments: Vec<FileMetadataId>,
    disable_search: Option<bool>,
    force_concise: Option<bool>,
    disable_memory: Option<bool>,
    enable_gmail_search: Option<bool>,
    enable_google_calendar_search: Option<bool>,
    enable_outlook_search: Option<bool>,
    enable_outlook_calendar_search: Option<bool>,
    enable_google_drive_search: Option<bool>,
}

/// Data assembled by the generated `ContinueConversationBuilder`.
#[derive(Builder)]
pub struct ContinueConversation<'a> {
    #[builder(start_fn)]
    client: &'a GrokClient,
    #[builder(start_fn)]
    conversation_id: ConversationId,
    #[builder(into)]
    message: Option<String>,
    parent_response_id: Option<ResponseId>,
    mode: Option<Mode>,
    #[builder(default)]
    attachments: Vec<FileMetadataId>,
    disable_search: Option<bool>,
    force_concise: Option<bool>,
    disable_memory: Option<bool>,
    enable_gmail_search: Option<bool>,
    enable_google_calendar_search: Option<bool>,
    enable_outlook_search: Option<bool>,
    enable_outlook_calendar_search: Option<bool>,
    enable_google_drive_search: Option<bool>,
}

/// Data assembled by the generated `ListConversationsBuilder`.
#[derive(Builder)]
pub struct ListConversations<'a> {
    #[builder(start_fn)]
    client: &'a GrokClient,
    #[builder(into)]
    page_token: Option<String>,
    page_size: Option<u32>,
}

/// Data assembled by the generated `GetConversationBuilder`.
#[derive(Builder)]
pub struct GetConversation<'a> {
    #[builder(start_fn)]
    client: &'a GrokClient,
    #[builder(start_fn)]
    conversation_id: ConversationId,
    #[builder(default)]
    include_messages: bool,
}

impl<'a> Conversations<'a> {
    pub fn start(self) -> StartConversationBuilder<'a> {
        StartConversation::builder(self.0)
    }

    pub fn continue_(self, id: &str) -> ContinueConversationBuilder<'a> {
        ContinueConversation::builder(self.0, ConversationId::new(id))
    }

    pub fn list(self) -> ListConversationsBuilder<'a> {
        ListConversations::builder(self.0)
    }

    pub fn get(self, id: &str) -> GetConversationBuilder<'a> {
        GetConversation::builder(self.0, ConversationId::new(id))
    }

    pub async fn response_node(
        &self,
        id: &str,
        include_threads: bool,
    ) -> Result<ResponseNodeResult> {
        let path = format!(
            "/rest/app-chat/conversations/{id}/response-node?includeThreads={include_threads}",
        );
        self.0.get_json(&path).await
    }

    pub async fn load_responses(
        &self,
        id: &str,
        response_ids: &[ResponseId],
    ) -> Result<LoadResponsesResponse> {
        let path = format!("/rest/app-chat/conversations/{id}/load-responses");
        self.0
            .post_json(&path, &json!({ "responseIds": response_ids }))
            .await
    }
}

impl<'a, S> StartConversationBuilder<'a, S>
where
    S: start_conversation_builder::State,
{
    pub async fn send(self) -> Result<StreamHandle> {
        self.build().send().await
    }
}

impl<'a> StartConversation<'a> {
    async fn send(self) -> Result<StreamHandle> {
        let overrides = self.overrides();
        let message = self
            .message
            .ok_or_else(|| Error::Config(ConfigError::MissingRequired("message")))?;
        let defaults = self.client.runtime().defaults().await;
        let body = build_new_conversation_request(
            message,
            self.attachments,
            self.mode,
            &defaults,
            overrides,
        );
        self.client
            .post_stream("/rest/app-chat/conversations/new", &body)
            .await
    }
}

impl<'a, S> ContinueConversationBuilder<'a, S>
where
    S: continue_conversation_builder::State,
{
    pub async fn send(self) -> Result<StreamHandle> {
        self.build().send().await
    }
}

impl<'a> ContinueConversation<'a> {
    async fn send(self) -> Result<StreamHandle> {
        let overrides = self.overrides();
        let message = self
            .message
            .ok_or_else(|| Error::Config(ConfigError::MissingRequired("message")))?;
        let defaults = self.client.runtime().defaults().await;
        let body = build_continue_conversation_request(
            message,
            self.parent_response_id,
            self.attachments,
            self.mode,
            &defaults,
            overrides,
        );
        let path = format!(
            "/rest/app-chat/conversations/{}/responses",
            self.conversation_id.as_str()
        );
        self.client.post_stream(&path, &body).await
    }
}

impl<'a, S> ListConversationsBuilder<'a, S>
where
    S: list_conversations_builder::State,
{
    pub async fn send(self) -> Result<ConversationList> {
        self.build().send().await
    }
}

impl<'a> ListConversations<'a> {
    async fn send(self) -> Result<ConversationList> {
        let mut path = String::from("/rest/app-chat/conversations");
        let mut params = Vec::<String>::new();
        if let Some(size) = self.page_size {
            params.push(format!("pageSize={size}"));
        }
        if let Some(token) = self.page_token {
            params.push(format!("pageToken={}", urlencoding(&token)));
        }
        if !params.is_empty() {
            path.push('?');
            path.push_str(&params.join("&"));
        }
        self.client.get_json(&path).await
    }
}

impl<'a, S> GetConversationBuilder<'a, S>
where
    S: get_conversation_builder::State,
{
    pub async fn send(self) -> Result<GetConversationOutput> {
        self.build().send().await
    }
}

impl<'a> GetConversation<'a> {
    async fn send(self) -> Result<GetConversationOutput> {
        let conversation = self.get_conversation().await?;
        if !self.include_messages {
            return Ok(GetConversationOutput {
                conversation,
                response_nodes: None,
                responses: None,
                message_count: 0,
                total_chars: 0,
            });
        }

        let node_result = Conversations(self.client)
            .response_node(self.conversation_id.as_str(), true)
            .await?;
        let ids = node_result
            .response_nodes
            .iter()
            .map(|node| node.response_id.clone())
            .collect::<Vec<ResponseId>>();
        let loaded_responses = if ids.is_empty() {
            Vec::new()
        } else {
            Conversations(self.client)
                .load_responses(self.conversation_id.as_str(), &ids)
                .await?
                .responses
        };
        let message_count = u32::try_from(loaded_responses.len())
            .map_err(|error| Error::StreamDecode(format!("response count overflow: {error}")))?;
        let total_chars = count_response_message_chars(&loaded_responses);
        let responses = loaded_responses
            .into_iter()
            .map(|response| serde_json::to_value(response).map_err(Error::Serde))
            .collect::<Result<Vec<serde_json::Value>>>()?;
        Ok(GetConversationOutput {
            conversation,
            response_nodes: Some(node_result.response_nodes),
            responses: Some(responses),
            message_count,
            total_chars,
        })
    }

    async fn get_conversation(&self) -> Result<Conversation> {
        let path = format!(
            "/rest/app-chat/conversations_v2/{}?includeWorkspaces=true&includeTaskResult=true",
            self.conversation_id.as_str()
        );
        let envelope = self.client.get_json::<ConversationEnvelope>(&path).await?;
        Ok(envelope.conversation)
    }
}

impl<'a> StartConversation<'a> {
    fn overrides(&self) -> BuilderOverrides {
        BuilderOverrides {
            disable_search: self.disable_search,
            force_concise: self.force_concise,
            disable_memory: self.disable_memory,
            enable_gmail_search: self.enable_gmail_search,
            enable_google_calendar_search: self.enable_google_calendar_search,
            enable_outlook_search: self.enable_outlook_search,
            enable_outlook_calendar_search: self.enable_outlook_calendar_search,
            enable_google_drive_search: self.enable_google_drive_search,
        }
    }
}

impl<'a> ContinueConversation<'a> {
    fn overrides(&self) -> BuilderOverrides {
        BuilderOverrides {
            disable_search: self.disable_search,
            force_concise: self.force_concise,
            disable_memory: self.disable_memory,
            enable_gmail_search: self.enable_gmail_search,
            enable_google_calendar_search: self.enable_google_calendar_search,
            enable_outlook_search: self.enable_outlook_search,
            enable_outlook_calendar_search: self.enable_outlook_calendar_search,
            enable_google_drive_search: self.enable_google_drive_search,
        }
    }
}

fn build_new_conversation_request(
    message: String,
    attachments: Vec<FileMetadataId>,
    mode: Option<Mode>,
    defaults: &ChatDefaults,
    overrides: BuilderOverrides,
) -> NewConversationRequest {
    NewConversationRequest {
        temporary: false,
        message,
        file_attachments: attachments,
        image_attachments: Vec::new(),
        mode_id: mode.unwrap_or(defaults.mode.clone()),
        options: build_chat_request_options(defaults, overrides),
    }
}

fn build_continue_conversation_request(
    message: String,
    parent_response_id: Option<ResponseId>,
    attachments: Vec<FileMetadataId>,
    mode: Option<Mode>,
    defaults: &ChatDefaults,
    overrides: BuilderOverrides,
) -> ContinueConversationRequest {
    ContinueConversationRequest {
        message,
        parent_response_id,
        file_attachments: attachments,
        image_attachments: Vec::new(),
        mode_id: mode,
        options: build_chat_request_options(defaults, overrides),
    }
}

pub(crate) fn count_response_message_chars(loaded_responses: &[LoadedResponse]) -> u64 {
    loaded_responses
        .iter()
        .filter_map(|response| response.extra.get("message"))
        .filter_map(|value| value.as_str())
        .map(|message| message.chars().count() as u64)
        .sum::<u64>()
}

#[derive(Default)]
struct BuilderOverrides {
    disable_search: Option<bool>,
    force_concise: Option<bool>,
    disable_memory: Option<bool>,
    enable_gmail_search: Option<bool>,
    enable_google_calendar_search: Option<bool>,
    enable_outlook_search: Option<bool>,
    enable_outlook_calendar_search: Option<bool>,
    enable_google_drive_search: Option<bool>,
}

fn build_chat_request_options(
    defaults: &ChatDefaults,
    overrides: BuilderOverrides,
) -> ChatRequestOptions {
    ChatRequestOptions {
        disable_search: overrides.disable_search.unwrap_or(defaults.disable_search),
        force_concise: overrides.force_concise.unwrap_or(defaults.force_concise),
        disable_memory: overrides.disable_memory.unwrap_or(defaults.disable_memory),
        tool_overrides: ToolOverrides {
            gmail_search: overrides.enable_gmail_search,
            google_calendar_search: overrides.enable_google_calendar_search,
            outlook_search: overrides.enable_outlook_search,
            outlook_calendar_search: overrides.enable_outlook_calendar_search,
            google_drive_search: overrides.enable_google_drive_search,
        },
        enable_image_generation: defaults.enable_image_generation,
        image_generation_count: defaults.image_generation_count,
        enable_side_by_side: defaults.enable_side_by_side,
        disable_text_follow_ups: defaults.disable_text_follow_ups,
        ..ChatRequestOptions::default()
    }
}

fn urlencoding(raw: &str) -> String {
    url::form_urlencoded::byte_serialize(raw.as_bytes()).collect::<String>()
}

impl GrokClient {
    #[must_use]
    pub fn conversations(&self) -> Conversations<'_> {
        Conversations(self)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{BuilderOverrides, ConversationEnvelope, build_chat_request_options};
    use crate::config::ChatDefaults;

    #[test]
    fn conversation_envelope_deserializes_wrapped_conversation() {
        let raw = r#"{
          "conversation": {
            "conversationId": "fake-conv-1",
            "title": "t",
            "starred": false,
            "createTime": "2026-04-18T00:00:00Z",
            "modifyTime": "2026-04-18T00:00:00Z",
            "systemPromptName": "",
            "temporary": false
          }
        }"#;

        let envelope = serde_json::from_str::<ConversationEnvelope>(raw).expect("parse envelope");

        assert_eq!(
            envelope.conversation.conversation_id.as_str(),
            "fake-conv-1"
        );
    }

    #[test]
    fn build_chat_request_options_applies_builder_overrides() {
        let defaults = ChatDefaults::default();
        let options = build_chat_request_options(
            &defaults,
            BuilderOverrides {
                disable_search: Some(true),
                force_concise: Some(true),
                disable_memory: Some(true),
                enable_gmail_search: Some(true),
                enable_google_calendar_search: None,
                enable_outlook_search: Some(false),
                enable_outlook_calendar_search: None,
                enable_google_drive_search: Some(true),
            },
        );

        let value = serde_json::to_value(options).expect("serialize options");

        assert_eq!(value.get("disableSearch"), Some(&json!(true)));
        assert_eq!(value.get("forceConcise"), Some(&json!(true)));
        assert_eq!(value.get("disableMemory"), Some(&json!(true)));
        assert_eq!(
            value
                .get("toolOverrides")
                .and_then(|value| value.get("gmailSearch")),
            Some(&json!(true))
        );
        assert_eq!(
            value
                .get("toolOverrides")
                .and_then(|value| value.get("outlookSearch")),
            Some(&json!(false))
        );
        assert_eq!(
            value
                .get("toolOverrides")
                .and_then(|value| value.get("googleDriveSearch")),
            Some(&json!(true))
        );
    }
}
