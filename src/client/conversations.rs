//! Conversation-oriented client API: list, get, start, continue, and hydrate.

use bon::Builder;
use serde::Deserialize;
use serde_json::json;

use crate::{
    client::{GrokClient, stream::StreamHandle},
    error::{Error, Result},
    models::{
        ChatOptions, Conversation, ConversationId, LoadResponsesResponse, LoadedResponse,
        ResponseId, ResponseNode,
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

/// Fresh-conversation request builder.
#[derive(Builder)]
#[builder(finish_fn = send_request)]
pub struct StartConversation<'a> {
    #[builder(start_fn)]
    client: &'a GrokClient,
    #[builder(start_fn, into)]
    message: String,
    #[builder(default)]
    options: ChatOptions,
}

/// Continuation-of-conversation request builder.
#[derive(Builder)]
#[builder(finish_fn = send_request)]
pub struct ContinueConversation<'a> {
    #[builder(start_fn)]
    client: &'a GrokClient,
    #[builder(start_fn)]
    conversation_id: ConversationId,
    #[builder(into)]
    message: Option<String>,
    parent_response_id: Option<ResponseId>,
    #[builder(default)]
    options: ChatOptions,
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
    /// Begin a new conversation with an initial user message.
    pub fn start(self, message: impl Into<String>) -> StartConversationBuilder<'a> {
        StartConversation::builder(self.0, message.into())
    }

    /// Continue an existing conversation by id.
    pub fn continue_(
        self,
        conversation_id: impl Into<ConversationId>,
    ) -> ContinueConversationBuilder<'a> {
        ContinueConversation::builder(self.0, conversation_id.into())
    }

    /// Build a paginated conversation-list request.
    pub fn list(self) -> ListConversationsBuilder<'a> {
        ListConversations::builder(self.0)
    }

    /// Build a conversation fetch request by id.
    pub fn get(self, conversation_id: impl Into<ConversationId>) -> GetConversationBuilder<'a> {
        GetConversation::builder(self.0, conversation_id.into())
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
    S: start_conversation_builder::IsComplete,
{
    /// Submit the request and return a live stream handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or if the streaming response
    /// cannot be decoded.
    pub async fn send(self) -> Result<StreamHandle> {
        let StartConversation {
            client,
            message,
            options,
        } = self.send_request();
        let defaults = client.runtime().defaults().await;
        let body = options.into_new_conversation_request(message, &defaults);
        client
            .post_stream("/rest/app-chat/conversations/new", &body)
            .await
    }
}

impl<'a, S> ContinueConversationBuilder<'a, S>
where
    S: continue_conversation_builder::IsComplete,
{
    /// Submit the request and return a live stream handle.
    ///
    /// # Errors
    ///
    /// Returns an error if the HTTP request fails or if the streaming response
    /// cannot be decoded.
    pub async fn send(self) -> Result<StreamHandle> {
        let ContinueConversation {
            client,
            conversation_id,
            message,
            parent_response_id,
            options,
        } = self.send_request();
        let defaults = client.runtime().defaults().await;
        let body =
            options.into_continue_conversation_request(message, parent_response_id, &defaults);
        let path = format!(
            "/rest/app-chat/conversations/{}/responses",
            conversation_id.as_str()
        );
        client.post_stream(&path, &body).await
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

pub(crate) fn count_response_message_chars(loaded_responses: &[LoadedResponse]) -> u64 {
    loaded_responses
        .iter()
        .filter_map(|response| response.extra.get("message"))
        .filter_map(|value| value.as_str())
        .map(|message| message.chars().count() as u64)
        .sum::<u64>()
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

    use super::ConversationEnvelope;
    use crate::{
        config::ChatDefaults,
        models::{ChatOptions, IntegrationFlags},
    };

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
    fn chat_options_into_wire_merges_defaults_and_overrides() {
        let defaults = ChatDefaults::default();
        let options = ChatOptions::builder()
            .disable_search(true)
            .force_concise(true)
            .disable_memory(true)
            .integrations(
                IntegrationFlags::builder()
                    .gmail(true)
                    .outlook(false)
                    .google_drive(true)
                    .build(),
            )
            .build();

        let value = serde_json::to_value(options.into_wire(&defaults)).expect("serialize options");

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
