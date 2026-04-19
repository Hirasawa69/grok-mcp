//! NDJSON stream parsing and collection for chat responses.
//!
//! grok.com streams two subtly different envelope shapes:
//!
//! * On a brand-new conversation, most assistant frames live under
//!   `result.response.<kind>`.
//! * On a continuation, assistant frames live under `result.<kind>` directly.
//!
//! A few event kinds — `result.conversation`, `result.title` — only appear at
//! the top level in both shapes. [`parse_envelope`] flattens both cases into
//! one semantic [`StreamEvent`].

use std::{pin::Pin, sync::LazyLock, time::Duration};

use futures_util::{Stream, StreamExt};
use regex::Regex;
use serde_json::{Map, Value};

use crate::{
    error::{Error, Result},
    models::{
        chat::{FinalChatResult, StreamEvent},
        common::{
            Citation, Conversation, ConversationId, FollowUpSuggestion, MessageTag, ModelResponse,
            ResponseId, ResponseStep, ToolUsageCard, ToolUsageCardId, UserResponse,
            WebSearchResult,
        },
    },
};

static INLINE_CITATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<grok:render\s[^>]*card_id="([^"]*)"[^>]*>.*?<argument\s+name="citation_id">(\d+)</argument>.*?</grok:render>"#,
    )
    .expect("inline citation regex should compile")
});

static TOOL_USAGE_CARD_XML_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?s)<xai:tool_usage_card>\s*<xai:tool_usage_card_id>([^<]+)</xai:tool_usage_card_id>\s*<xai:tool_name>([^<]+)</xai:tool_name>\s*<xai:tool_args><!\[CDATA\[(.*?)\]\]></xai:tool_args>\s*</xai:tool_usage_card>"#,
    )
    .expect("tool usage card regex should compile")
});

/// Erased byte stream. Internal-only because the reqwest wrapping happens at
/// construction time; tests inject a stream already wrapped in our `Error`.
type ByteStream = Pin<Box<dyn Stream<Item = Result<bytes::Bytes>> + Send>>;

/// Live handle over an in-flight chat stream.
///
/// `next_event` pulls one normalized [`StreamEvent`] at a time; `collect_final`
/// is the common-case drain that returns a [`FinalChatResult`] when the stream
/// reaches a terminal `modelResponse`.
pub struct StreamHandle {
    inner: LineReader,
}

impl StreamHandle {
    pub(crate) fn from_response(response: reqwest::Response, idle_timeout: Duration) -> Self {
        let byte_stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(Error::Transport));
        Self {
            inner: LineReader::new(Box::pin(byte_stream), idle_timeout),
        }
    }

    /// Construct a handle from an already-wrapped byte stream. Used by tests
    /// that want to exercise the parsing + idle-timeout paths without a real
    /// HTTP response.
    #[cfg(test)]
    pub(crate) fn from_byte_stream(bytes: ByteStream, idle_timeout: Duration) -> Self {
        Self {
            inner: LineReader::new(bytes, idle_timeout),
        }
    }

    /// Parse and return the next semantic event, or `Ok(None)` at EOF.
    ///
    /// # Errors
    ///
    /// * [`Error::Transport`] on underlying reqwest errors.
    /// * [`Error::StreamDecode`] if a line fails to parse as JSON or envelope.
    pub async fn next_event(&mut self) -> Result<Option<StreamEvent>> {
        loop {
            let Some(line) = self.inner.next_line().await? else {
                return Ok(None);
            };
            if line.trim().is_empty() {
                continue;
            }
            let value = serde_json::from_str::<Value>(&line)
                .map_err(|error| Error::StreamDecode(format!("line json: {error}")))?;
            let event = parse_envelope(value)?;
            return Ok(Some(event));
        }
    }

    /// Drain the stream just far enough to capture the conversation and
    /// response identifiers, then stop reading.
    ///
    /// # Errors
    ///
    /// Returns [`Error::StreamEnded`] if the stream ends before the required id
    /// fields are observed. When `seed_conversation_id` is provided (the
    /// continuation path), the stream only needs to yield a `response_id`.
    pub async fn drain_ids(
        &mut self,
        seed_conversation_id: Option<ConversationId>,
    ) -> Result<(ConversationId, ResponseId, Option<ResponseId>)> {
        let mut conversation_id = seed_conversation_id;
        let mut response_id = None;
        let mut parent_response_id = None;

        loop {
            let Some(event) = self.next_event().await? else {
                return Err(Error::StreamEnded);
            };

            match event {
                StreamEvent::Conversation(conversation) => {
                    conversation_id = Some(conversation.conversation_id.clone());
                }
                StreamEvent::UserResponse(user_response) => {
                    if parent_response_id.is_none() {
                        parent_response_id = user_response.parent_response_id.clone();
                    }
                }
                StreamEvent::ModelResponse(model_response) => {
                    if response_id.is_none() {
                        response_id = Some(model_response.response_id.clone());
                    }
                    if parent_response_id.is_none() {
                        parent_response_id = model_response.parent_response_id.clone();
                    }
                }
                StreamEvent::Token {
                    response_id: token_response_id,
                    ..
                }
                | StreamEvent::UiLayout {
                    response_id: token_response_id,
                    ..
                }
                | StreamEvent::LlmInfo {
                    response_id: token_response_id,
                    ..
                }
                | StreamEvent::ProgressReport {
                    response_id: token_response_id,
                    ..
                }
                | StreamEvent::WebSearchResults {
                    response_id: token_response_id,
                    ..
                }
                | StreamEvent::FinalMetadata {
                    response_id: token_response_id,
                    ..
                } => {
                    if response_id.is_none() {
                        response_id = token_response_id;
                    }
                }
                StreamEvent::ToolUsageCard(_)
                | StreamEvent::Title { .. }
                | StreamEvent::Unknown(_) => (),
            }

            if let (Some(conversation_id), Some(response_id)) =
                (conversation_id.clone(), response_id.clone())
            {
                return Ok((conversation_id, response_id, parent_response_id));
            }
        }
    }

    /// Drain the stream into a single [`FinalChatResult`].
    ///
    /// The `seed_conversation_id` is used when the stream never emits a
    /// `conversation` event (e.g. continuation requests on an existing
    /// conversation).
    ///
    /// # Errors
    ///
    /// * [`Error::StreamEnded`] if the stream ends before a `modelResponse` arrives.
    pub async fn collect_final(
        mut self,
        seed_conversation_id: Option<ConversationId>,
    ) -> Result<FinalChatResult> {
        let mut collector = Collector::new(seed_conversation_id);
        while let Some(event) = self.next_event().await? {
            collector.ingest(event);
        }
        collector.finish()
    }
}

/// Parse a single NDJSON frame value into a [`StreamEvent`]. Exposed for tests.
pub fn parse_envelope(value: Value) -> Result<StreamEvent> {
    let Value::Object(mut outer) = value else {
        return Err(Error::StreamDecode(
            "top-level frame is not a JSON object".to_owned(),
        ));
    };
    let Some(result) = outer.remove("result") else {
        return Ok(StreamEvent::Unknown(Value::Object(outer)));
    };
    let Value::Object(mut result_obj) = result else {
        return Ok(StreamEvent::Unknown(result));
    };

    if let Some(conversation) = result_obj.remove("conversation") {
        let conv = serde_json::from_value::<Conversation>(conversation)
            .map_err(|error| Error::StreamDecode(format!("conversation: {error}")))?;
        return Ok(StreamEvent::Conversation(Box::new(conv)));
    }

    if let Some(title) = result_obj.remove("title") {
        let new_title = extract_string_field(&title, "newTitle").unwrap_or_default();
        return Ok(StreamEvent::Title { new_title });
    }

    if let Some(response) = result_obj.remove("response") {
        return parse_response_frame(response);
    }

    parse_flat_frame(Value::Object(result_obj))
}

fn parse_response_frame(value: Value) -> Result<StreamEvent> {
    let Value::Object(mut obj) = value else {
        return Ok(StreamEvent::Unknown(value));
    };
    classify_flat(&mut obj)
}

fn parse_flat_frame(value: Value) -> Result<StreamEvent> {
    let Value::Object(mut obj) = value else {
        return Ok(StreamEvent::Unknown(value));
    };
    classify_flat(&mut obj)
}

fn classify_flat(obj: &mut serde_json::Map<String, Value>) -> Result<StreamEvent> {
    let meta = MetaFields::extract(obj);

    if let Some(user_response) = obj.remove("userResponse") {
        let payload = serde_json::from_value::<UserResponse>(user_response)
            .map_err(|error| Error::StreamDecode(format!("userResponse: {error}")))?;
        return Ok(StreamEvent::UserResponse(Box::new(payload)));
    }

    if let Some(ui_layout) = obj.remove("uiLayout") {
        return Ok(StreamEvent::UiLayout {
            response_id: meta.response_id,
            layout: ui_layout,
        });
    }

    if let Some(llm_info) = obj.remove("llmInfo") {
        let model_hash = extract_string_field(&llm_info, "modelHash").unwrap_or_default();
        return Ok(StreamEvent::LlmInfo {
            response_id: meta.response_id,
            model_hash,
        });
    }

    if let Some(progress_report) = obj.remove("progressReport") {
        let category = extract_string_field(&progress_report, "category").unwrap_or_default();
        let state = extract_string_field(&progress_report, "state").unwrap_or_default();
        let message = extract_string_field(&progress_report, "message");
        return Ok(StreamEvent::ProgressReport {
            response_id: meta.response_id,
            category,
            state,
            message,
        });
    }

    if let Some(token_value) = obj.remove("token") {
        let token = match token_value {
            Value::String(text) => text,
            other => other.to_string(),
        };
        return Ok(StreamEvent::Token {
            response_id: meta.response_id,
            token,
            message_tag: meta.message_tag,
            message_step_id: meta.message_step_id,
            is_thinking: meta.is_thinking.unwrap_or(false),
            is_soft_stop: meta.is_soft_stop.unwrap_or(false),
            tool_usage_card_id: meta.tool_usage_card_id,
        });
    }

    if let Some(card) = obj.remove("toolUsageCard") {
        let payload = serde_json::from_value::<ToolUsageCard>(card)
            .map_err(|error| Error::StreamDecode(format!("toolUsageCard: {error}")))?;
        return Ok(StreamEvent::ToolUsageCard(Box::new(payload)));
    }

    if let Some(search) = obj.remove("webSearchResults") {
        let results = extract_web_search_results(search)?;
        return Ok(StreamEvent::WebSearchResults {
            response_id: meta.response_id,
            tool_usage_card_id: meta.tool_usage_card_id,
            message_tag: meta.message_tag,
            results,
        });
    }

    if let Some(final_meta) = obj.remove("finalMetadata") {
        let (follow_up_suggestions, disclaimer) = extract_final_metadata(final_meta)?;
        return Ok(StreamEvent::FinalMetadata {
            response_id: meta.response_id,
            follow_up_suggestions,
            disclaimer,
        });
    }

    if let Some(model_response) = obj.remove("modelResponse") {
        let payload = serde_json::from_value::<ModelResponse>(model_response)
            .map_err(|error| Error::StreamDecode(format!("modelResponse: {error}")))?;
        return Ok(StreamEvent::ModelResponse(Box::new(payload)));
    }

    if let Some(title) = obj.remove("title") {
        let new_title = extract_string_field(&title, "newTitle").unwrap_or_default();
        return Ok(StreamEvent::Title { new_title });
    }

    let leftover = std::mem::take(obj);
    Ok(StreamEvent::Unknown(Value::Object(leftover)))
}

/// Companion metadata fields that appear alongside payload kinds on the same frame.
struct MetaFields {
    response_id: Option<ResponseId>,
    is_thinking: Option<bool>,
    is_soft_stop: Option<bool>,
    message_tag: Option<MessageTag>,
    message_step_id: Option<u32>,
    tool_usage_card_id: Option<ToolUsageCardId>,
}

impl MetaFields {
    fn extract(obj: &mut serde_json::Map<String, Value>) -> Self {
        Self {
            response_id: obj
                .remove("responseId")
                .and_then(|value| value.as_str().map(ResponseId::new)),
            is_thinking: obj.remove("isThinking").and_then(|value| value.as_bool()),
            is_soft_stop: obj.remove("isSoftStop").and_then(|value| value.as_bool()),
            message_tag: obj
                .remove("messageTag")
                .and_then(|value| serde_json::from_value::<MessageTag>(value).ok()),
            message_step_id: obj
                .remove("messageStepId")
                .and_then(|value| value.as_u64().and_then(|v| u32::try_from(v).ok())),
            tool_usage_card_id: obj
                .remove("toolUsageCardId")
                .and_then(|value| value.as_str().map(ToolUsageCardId::new)),
        }
    }
}

/// Coerce any JSON value into an object for the typed `unknown_events` list.
///
/// JSON Schema draft 2020-12 permits bare-value schemas (`{ "type": true }`),
/// but some strict MCP clients (opencode 1.4.7+) reject them via Zod. Keeping
/// the typed surface as `Vec<Map<...>>` guarantees `{ "type": "object" }`
/// everywhere; any non-object frame gets tucked under a `__non_object` key so
/// no information is lost.
fn wrap_as_object(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => {
            let mut wrapped = serde_json::Map::new();
            wrapped.insert("__non_object".to_owned(), other);
            wrapped
        }
    }
}

fn extract_citation_from_unknown_event(value: &Value) -> Option<Citation> {
    let json_data = value
        .as_object()?
        .get("cardAttachment")?
        .as_object()?
        .get("jsonData")?
        .as_str()?;
    let decoded = serde_json::from_str::<Value>(json_data).ok()?;
    let decoded = decoded.as_object()?;
    let card_id = decoded.get("id")?.as_str()?.to_owned();

    Some(build_citation(card_id, decoded))
}

fn extract_citation_from_card_attachment_value(value: &Value) -> Option<Citation> {
    let decoded = match value {
        Value::String(raw) => serde_json::from_str::<Value>(raw).ok()?,
        Value::Object(_) => value.clone(),
        _ => return None,
    };
    let decoded = decoded.as_object()?;
    let decoded = if let Some(json_data) = decoded.get("jsonData").and_then(|value| value.as_str())
    {
        let value = serde_json::from_str::<Value>(json_data).ok()?;
        value.as_object()?.clone()
    } else {
        decoded.clone()
    };
    let card_id = decoded.get("id")?.as_str()?.to_owned();

    Some(build_citation(card_id, &decoded))
}

fn build_citation(card_id: String, decoded: &Map<String, Value>) -> Citation {
    Citation {
        card_id,
        citation_id: None,
        card_type: decoded
            .get("cardType")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        url: decoded
            .get("url")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
    }
}

pub(crate) fn extract_citations_from_card_attachments(value: &Value) -> Vec<Citation> {
    value
        .as_array()
        .into_iter()
        .flat_map(|items| {
            items
                .iter()
                .filter_map(extract_citation_from_card_attachment_value)
        })
        .collect::<Vec<Citation>>()
}

pub(crate) fn merge_citations_from_message(
    message: &str,
    stream_citations: &[Citation],
) -> Vec<Citation> {
    INLINE_CITATION_REGEX
        .captures_iter(message)
        .filter_map(|captures| {
            let card_id = captures.get(1)?.as_str().to_owned();
            let citation_id = captures.get(2)?.as_str().to_owned();
            let mut citation = stream_citations
                .iter()
                .find(|citation| citation.card_id == card_id)
                .cloned()
                .unwrap_or(Citation {
                    card_id,
                    citation_id: None,
                    card_type: None,
                    url: None,
                });
            citation.citation_id = Some(citation_id);
            Some(citation)
        })
        .collect::<Vec<Citation>>()
}

fn extract_string_field(value: &Value, key: &str) -> Option<String> {
    value
        .as_object()
        .and_then(|obj| obj.get(key))
        .and_then(|inner| inner.as_str().map(str::to_owned))
}

pub(crate) fn extract_web_search_results(value: Value) -> Result<Vec<WebSearchResult>> {
    let array = match value {
        Value::Object(map) => match map.get("results") {
            Some(Value::Array(items)) => items.clone(),
            _ => return Ok(Vec::new()),
        },
        Value::Array(items) => items,
        _ => return Ok(Vec::new()),
    };
    serde_json::from_value::<Vec<WebSearchResult>>(Value::Array(array))
        .map_err(|error| Error::StreamDecode(format!("webSearchResults: {error}")))
}

fn extract_final_metadata(value: Value) -> Result<(Vec<FollowUpSuggestion>, Option<String>)> {
    let Value::Object(obj) = value else {
        return Ok((Vec::new(), None));
    };
    let disclaimer = obj
        .get("disclaimer")
        .and_then(|v| v.as_str())
        .map(str::to_owned);
    let Some(suggestions_value) = obj.get("followUpSuggestions") else {
        return Ok((Vec::new(), disclaimer));
    };
    let suggestions = serde_json::from_value::<Vec<FollowUpSuggestion>>(suggestions_value.clone())
        .map_err(|error| Error::StreamDecode(format!("followUpSuggestions: {error}")))?;
    Ok((suggestions, disclaimer))
}

/// In-process drain of a [`StreamHandle`] into a [`FinalChatResult`].
///
/// Exposed so callers that need to observe individual events (e.g. forwarding
/// progress notifications through MCP) can drive the collector themselves
/// instead of handing the whole stream to [`StreamHandle::collect_final`].
pub struct Collector {
    conversation_id: Option<ConversationId>,
    response_id: Option<ResponseId>,
    parent_response_id: Option<ResponseId>,
    message: Option<String>,
    thinking: String,
    citations: Vec<Citation>,
    tool_usage_cards: Vec<ToolUsageCard>,
    web_search_results: Vec<WebSearchResult>,
    follow_up_suggestions: Vec<FollowUpSuggestion>,
    title: Option<String>,
    steps: Option<Vec<ResponseStep>>,
    unknown_events: Vec<Map<String, Value>>,
    saw_model_response: bool,
}

impl Collector {
    /// Create a collector pre-seeded with a conversation id (typical for a
    /// continuation, which never emits a `conversation` event).
    #[must_use]
    pub fn new(seed_conversation_id: Option<ConversationId>) -> Self {
        Self {
            conversation_id: seed_conversation_id,
            response_id: None,
            parent_response_id: None,
            message: None,
            thinking: String::new(),
            citations: Vec::new(),
            tool_usage_cards: Vec::new(),
            web_search_results: Vec::new(),
            follow_up_suggestions: Vec::new(),
            title: None,
            steps: None,
            unknown_events: Vec::new(),
            saw_model_response: false,
        }
    }

    /// Absorb a single stream event into the running collection.
    pub fn ingest(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Conversation(conv) => {
                self.conversation_id = Some(conv.conversation_id.clone());
            }
            StreamEvent::UserResponse(user) => {
                if self.parent_response_id.is_none() {
                    self.parent_response_id = user.parent_response_id.clone();
                }
            }
            StreamEvent::Token {
                token,
                message_tag,
                is_thinking,
                ..
            } => {
                // `modelResponse.message` carries the final answer verbatim, so
                // only thinking-trace fragments need to be accumulated here.
                if is_thinking
                    || matches!(
                        message_tag.as_ref(),
                        Some(MessageTag::Header | MessageTag::Summary)
                    )
                {
                    self.thinking.push_str(&token);
                    if is_thinking {
                        self.extract_tool_usage_cards_from_thinking();
                    }
                }
            }
            StreamEvent::ToolUsageCard(card) => {
                self.push_tool_usage_card(*card);
            }
            StreamEvent::WebSearchResults { results, .. } => {
                self.web_search_results.extend(results);
            }
            StreamEvent::FinalMetadata {
                follow_up_suggestions,
                ..
            } => {
                self.follow_up_suggestions = follow_up_suggestions;
            }
            StreamEvent::ModelResponse(model_response) => {
                self.response_id = Some(model_response.response_id.clone());
                if self.parent_response_id.is_none() {
                    self.parent_response_id = model_response.parent_response_id.clone();
                }
                self.message = Some(model_response.message.clone());
                self.saw_model_response = true;
            }
            StreamEvent::Title { new_title } => {
                self.title = Some(new_title);
            }
            StreamEvent::Unknown(value) => {
                if let Some(citation) = extract_citation_from_unknown_event(&value) {
                    self.upsert_citation(citation);
                }
                self.unknown_events.push(wrap_as_object(value));
            }
            StreamEvent::UiLayout { .. }
            | StreamEvent::LlmInfo { .. }
            | StreamEvent::ProgressReport { .. } => (),
        }
    }

    /// Convert the accumulated state into a [`FinalChatResult`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::StreamEnded`] if no `modelResponse` event was ever
    /// ingested (or if conversation/response ids are still unknown).
    pub fn finish(self) -> Result<FinalChatResult> {
        if !self.saw_model_response {
            return Err(Error::StreamEnded);
        }
        let conversation_id = self.conversation_id.ok_or(Error::StreamEnded)?;
        let response_id = self.response_id.ok_or(Error::StreamEnded)?;
        let message = self.message.unwrap_or_default();
        let citations = merge_citations_from_message(&message, &self.citations);
        let thinking = if self.thinking.is_empty() {
            None
        } else {
            Some(self.thinking)
        };
        Ok(FinalChatResult {
            conversation_id,
            response_id,
            parent_response_id: self.parent_response_id,
            message,
            thinking,
            citations,
            tool_usage_cards: self.tool_usage_cards,
            web_search_results: self.web_search_results,
            follow_up_suggestions: self.follow_up_suggestions,
            title: self.title,
            steps: self.steps,
            agent_messages: Vec::new(),
            unknown_events: self.unknown_events,
        })
    }

    fn extract_tool_usage_cards_from_thinking(&mut self) {
        let cards = TOOL_USAGE_CARD_XML_REGEX
            .captures_iter(&self.thinking)
            .filter_map(|captures| {
                let tool_usage_card_id = captures.get(1).map(|capture| capture.as_str())?;
                let tool_name = captures.get(2).map(|capture| capture.as_str())?;
                let tool_args_json = captures.get(3).map(|capture| capture.as_str())?;
                let Ok(args) = serde_json::from_str::<Value>(tool_args_json) else {
                    return None;
                };

                let mut extra = Map::new();
                extra.insert("tool_name".to_owned(), Value::String(tool_name.to_owned()));
                extra.insert("args".to_owned(), args);

                Some(ToolUsageCard {
                    tool_usage_card_id: ToolUsageCardId::new(tool_usage_card_id),
                    extra,
                })
            })
            .collect::<Vec<ToolUsageCard>>();

        for card in cards {
            self.push_tool_usage_card(card);
        }
    }

    fn push_tool_usage_card(&mut self, card: ToolUsageCard) {
        if self
            .tool_usage_cards
            .iter()
            .any(|existing| existing.tool_usage_card_id == card.tool_usage_card_id)
        {
            return;
        }
        self.tool_usage_cards.push(card);
    }

    fn upsert_citation(&mut self, citation: Citation) {
        if let Some(existing) = self
            .citations
            .iter_mut()
            .find(|existing| existing.card_id == citation.card_id)
        {
            if existing.citation_id.is_none() {
                existing.citation_id = citation.citation_id;
            }
            if existing.card_type.is_none() {
                existing.card_type = citation.card_type;
            }
            if existing.url.is_none() {
                existing.url = citation.url;
            }
            return;
        }
        self.citations.push(citation);
    }
}

/// Stateful reader that emits one `\n`-delimited line at a time.
///
/// Each chunk-read is wrapped in [`tokio::time::timeout`] using the configured
/// idle budget. If the upstream stalls for longer than that, `next_line`
/// returns [`Error::StreamIdleTimeout`] rather than hanging until the outer
/// request timeout fires. A zero-duration budget disables the check.
struct LineReader {
    bytes: ByteStream,
    buffer: Vec<u8>,
    idle_timeout: Duration,
}

impl LineReader {
    fn new(bytes: ByteStream, idle_timeout: Duration) -> Self {
        Self {
            bytes,
            buffer: Vec::new(),
            idle_timeout,
        }
    }

    async fn next_line(&mut self) -> Result<Option<String>> {
        loop {
            if let Some(position) = self.buffer.iter().position(|byte| *byte == b'\n') {
                let line_bytes = self.buffer.drain(..=position).collect::<Vec<u8>>();
                let without_newline = &line_bytes[..line_bytes.len() - 1];
                let line = String::from_utf8(without_newline.to_vec())
                    .map_err(|error| Error::StreamDecode(format!("non-utf8 line: {error}")))?;
                return Ok(Some(line));
            }
            let next_chunk = self.next_chunk_with_idle_timeout().await?;
            match next_chunk {
                Some(chunk) => self.buffer.extend_from_slice(&chunk),
                None => {
                    if self.buffer.is_empty() {
                        return Ok(None);
                    }
                    let tail = std::mem::take(&mut self.buffer);
                    let line = String::from_utf8(tail)
                        .map_err(|error| Error::StreamDecode(format!("non-utf8 tail: {error}")))?;
                    return Ok(Some(line));
                }
            }
        }
    }

    async fn next_chunk_with_idle_timeout(&mut self) -> Result<Option<bytes::Bytes>> {
        if self.idle_timeout.is_zero() {
            return match self.bytes.next().await {
                Some(Ok(chunk)) => Ok(Some(chunk)),
                Some(Err(error)) => Err(error),
                None => Ok(None),
            };
        }
        match tokio::time::timeout(self.idle_timeout, self.bytes.next()).await {
            Ok(Some(Ok(chunk))) => Ok(Some(chunk)),
            Ok(Some(Err(error))) => Err(error),
            Ok(None) => Ok(None),
            Err(_elapsed) => Err(Error::StreamIdleTimeout {
                timeout_seconds: self.idle_timeout.as_secs(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn final_model_response(message: &str) -> StreamEvent {
        StreamEvent::ModelResponse(Box::new(ModelResponse {
            response_id: ResponseId::new("r1"),
            message: message.to_owned(),
            sender: None,
            parent_response_id: None,
            create_time: None,
            thinking_start_time: None,
            thinking_end_time: None,
            extra: Map::new(),
        }))
    }

    #[test]
    fn wrapped_envelope_model_response_parses() {
        let raw = r#"{"result":{"response":{"modelResponse":{"responseId":"r1","message":"hello","sender":"ASSISTANT","createTime":"2026-04-17T00:00:00Z","parentResponseId":"p1","manual":false,"partial":false,"shared":false,"query":"","queryType":"","webSearchResults":[],"xpostIds":[],"xposts":[],"generatedImageUrls":[],"imageAttachments":[],"fileAttachments":[],"cardAttachmentsJson":[],"fileUris":[],"fileAttachmentsMetadata":[],"isControl":false,"steps":[],"imageEditUris":[],"mediaTypes":[],"webpageUrls":[],"metadata":{},"toolResponses":[],"model":"","ragResults":[],"citedRagResults":[],"searchProductResults":[],"connectorSearchResults":[],"collectionSearchResults":[],"streamErrors":[],"citedWebSearchResults":[],"citedXposts":[],"citedConnectorSearchResults":[],"citedCollectionSearchResults":[]},"isThinking":false,"isSoftStop":false,"responseId":"r1"}}}"#;
        let value = serde_json::from_str::<Value>(raw).expect("json");
        let event = parse_envelope(value).expect("parse");
        let StreamEvent::ModelResponse(model) = event else {
            panic!("expected modelResponse");
        };
        assert_eq!(model.message, "hello");
        assert_eq!(model.response_id.as_str(), "r1");
    }

    #[test]
    fn flat_envelope_token_parses() {
        let raw = r#"{"result":{"token":"hi","messageTag":"final","responseId":"r2","isThinking":false,"isSoftStop":false}}"#;
        let value = serde_json::from_str::<Value>(raw).expect("json");
        let event = parse_envelope(value).expect("parse");
        let StreamEvent::Token {
            token,
            response_id,
            message_tag,
            is_thinking,
            ..
        } = event
        else {
            panic!("expected token event");
        };
        assert_eq!(token, "hi");
        assert_eq!(response_id.expect("id").as_str(), "r2");
        assert!(matches!(message_tag, Some(MessageTag::Final)));
        assert!(!is_thinking);
    }

    #[test]
    fn title_event_parses() {
        let raw = r#"{"result":{"title":{"newTitle":"Hello"}}}"#;
        let value = serde_json::from_str::<Value>(raw).expect("json");
        let event = parse_envelope(value).expect("parse");
        let StreamEvent::Title { new_title } = event else {
            panic!("expected title");
        };
        assert_eq!(new_title, "Hello");
    }

    #[test]
    fn collector_drains_minimal_stream() {
        let conv = r#"{"result":{"conversation":{"conversationId":"c1","title":"t","starred":false,"createTime":"2026-04-17T00:00:00Z","modifyTime":"2026-04-17T00:00:00Z","systemPromptName":"","temporary":false}}}"#;
        let token = r#"{"result":{"response":{"token":"hello ","messageTag":"final","responseId":"r1","isThinking":false,"isSoftStop":false}}}"#;
        let model = r#"{"result":{"response":{"modelResponse":{"responseId":"r1","message":"hello","sender":"ASSISTANT","parentResponseId":"p1"}}}}"#;

        let events = [conv, token, model]
            .into_iter()
            .map(|raw| {
                let value = serde_json::from_str::<Value>(raw).expect("json");
                parse_envelope(value).expect("parse")
            })
            .collect::<Vec<StreamEvent>>();

        let mut collector = Collector::new(None);
        for event in events {
            collector.ingest(event);
        }
        let result = collector.finish().expect("final");
        assert_eq!(result.conversation_id.as_str(), "c1");
        assert_eq!(result.response_id.as_str(), "r1");
        assert_eq!(result.message, "hello");
        assert_eq!(result.parent_response_id.expect("parent").as_str(), "p1");
        assert!(result.citations.is_empty());
        assert!(result.agent_messages.is_empty());
    }

    #[test]
    fn collector_extracts_citations_from_message_and_unknown_events() {
        let mut collector = Collector::new(Some(ConversationId::new("c1")));
        collector.ingest(StreamEvent::Unknown(serde_json::json!({
            "cardAttachment": {
                "jsonData": "{\"id\":\"card-1\",\"type\":\"render_inline_citation\",\"cardType\":\"citation_card\",\"url\":\"https://example.com/one\"}"
            }
        })));
        collector.ingest(StreamEvent::Unknown(serde_json::json!({
            "cardAttachment": {
                "jsonData": "{\"id\":\"card-2\",\"type\":\"render_inline_citation\",\"cardType\":\"citation_card\",\"url\":\"https://example.com/two\"}"
            }
        })));
        collector.ingest(StreamEvent::ModelResponse(Box::new(ModelResponse {
            response_id: ResponseId::new("r1"),
            message: concat!(
                "First <grok:render card_id=\"card-1\" card_type=\"citation_card\" type=\"render_inline_citation\">",
                "<argument name=\"citation_id\">9</argument></grok:render>",
                " second <grok:render card_id=\"card-2\" card_type=\"citation_card\" type=\"render_inline_citation\">",
                "<argument name=\"citation_id\">10</argument></grok:render>"
            )
            .to_owned(),
            sender: None,
            parent_response_id: Some(ResponseId::new("p1")),
            create_time: None,
            thinking_start_time: None,
            thinking_end_time: None,
            extra: Map::new(),
        })));

        let result = collector.finish().expect("final result");

        assert_eq!(result.citations.len(), 2);
        assert_eq!(result.citations[0].card_id, "card-1");
        assert_eq!(result.citations[0].citation_id.as_deref(), Some("9"));
        assert_eq!(
            result.citations[0].url.as_deref(),
            Some("https://example.com/one")
        );
        assert_eq!(result.citations[1].card_id, "card-2");
        assert_eq!(result.citations[1].citation_id.as_deref(), Some("10"));
        assert_eq!(
            result.citations[1].url.as_deref(),
            Some("https://example.com/two")
        );
    }

    #[test]
    fn collector_extracts_tool_usage_card_from_thinking_xml() {
        let mut collector = Collector::new(Some(ConversationId::new("c1")));
        collector.ingest(StreamEvent::Token {
            response_id: Some(ResponseId::new("r1")),
            token: concat!(
                "<xai:tool_usage_card>",
                "<xai:tool_usage_card_id>tool-card-1</xai:tool_usage_card_id>",
                "<xai:tool_name>web_search</xai:tool_name>",
                "<xai:tool_args><![CDATA[{\"query\":\"rust regex\"}]]></xai:tool_args>",
                "</xai:tool_usage_card>"
            )
            .to_owned(),
            message_tag: Some(MessageTag::ToolUsageCard),
            message_step_id: None,
            is_thinking: true,
            is_soft_stop: false,
            tool_usage_card_id: None,
        });
        collector.ingest(final_model_response("done"));

        let result = collector.finish().expect("final result");

        assert_eq!(result.tool_usage_cards.len(), 1);
        assert_eq!(
            result.tool_usage_cards[0].tool_usage_card_id.as_str(),
            "tool-card-1"
        );
        assert_eq!(
            result.tool_usage_cards[0]
                .extra
                .get("tool_name")
                .and_then(|value| value.as_str()),
            Some("web_search")
        );
        assert_eq!(
            result.tool_usage_cards[0]
                .extra
                .get("args")
                .and_then(|value| value.as_object())
                .and_then(|value| value.get("query"))
                .and_then(|value| value.as_str()),
            Some("rust regex")
        );
    }

    #[test]
    fn collector_deduplicates_xml_tool_usage_cards_against_structured_frames() {
        let mut collector = Collector::new(Some(ConversationId::new("c1")));
        collector.ingest(StreamEvent::ToolUsageCard(Box::new(ToolUsageCard {
            tool_usage_card_id: ToolUsageCardId::new("tool-card-1"),
            extra: serde_json::from_value::<Map<String, Value>>(serde_json::json!({
                "webSearch": {
                    "args": {
                        "query": "structured"
                    }
                }
            }))
            .expect("map"),
        })));
        collector.ingest(StreamEvent::Token {
            response_id: Some(ResponseId::new("r1")),
            token: concat!(
                "<xai:tool_usage_card>",
                "<xai:tool_usage_card_id>tool-card-1</xai:tool_usage_card_id>",
                "<xai:tool_name>web_search</xai:tool_name>",
                "<xai:tool_args><![CDATA[{\"query\":\"xml\"}]]></xai:tool_args>",
                "</xai:tool_usage_card>"
            )
            .to_owned(),
            message_tag: Some(MessageTag::ToolUsageCard),
            message_step_id: None,
            is_thinking: true,
            is_soft_stop: false,
            tool_usage_card_id: Some(ToolUsageCardId::new("tool-card-1")),
        });
        collector.ingest(final_model_response("done"));

        let result = collector.finish().expect("final result");

        assert_eq!(result.tool_usage_cards.len(), 1);
        assert!(result.tool_usage_cards[0].extra.contains_key("webSearch"));
        assert!(!result.tool_usage_cards[0].extra.contains_key("tool_name"));
    }

    #[test]
    fn collector_keeps_citation_id_when_card_attachment_is_missing() {
        let mut collector = Collector::new(Some(ConversationId::new("c1")));
        collector.ingest(StreamEvent::ModelResponse(Box::new(ModelResponse {
            response_id: ResponseId::new("r1"),
            message: concat!(
                "Only inline <grok:render card_id=\"card-9\" card_type=\"citation_card\" type=\"render_inline_citation\">",
                "<argument name=\"citation_id\">42</argument></grok:render>"
            )
            .to_owned(),
            sender: None,
            parent_response_id: Some(ResponseId::new("p1")),
            create_time: None,
            thinking_start_time: None,
            thinking_end_time: None,
            extra: Map::new(),
        })));

        let result = collector.finish().expect("final result");

        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].card_id, "card-9");
        assert_eq!(result.citations[0].citation_id.as_deref(), Some("42"));
        assert_eq!(result.citations[0].card_type, None);
        assert_eq!(result.citations[0].url, None);
    }

    #[tokio::test(start_paused = true)]
    async fn drain_ids_returns_before_consuming_later_frames() {
        use futures_util::stream;

        let conv_line = concat!(
            r#"{"result":{"conversation":{"conversationId":"c1","title":"t","starred":false,"createTime":"2026-04-17T00:00:00Z","modifyTime":"2026-04-17T00:00:00Z","systemPromptName":"","temporary":false}}}"#,
            "\n",
        );
        let user_line = concat!(
            r#"{"result":{"response":{"userResponse":{"responseId":"user-echo-1","message":"hello","parentResponseId":"p1"}}}}"#,
            "\n",
        );
        let token_line = concat!(
            r#"{"result":{"response":{"token":"working","messageTag":"summary","responseId":"assistant-1","isThinking":true,"isSoftStop":false}}}"#,
            "\n",
        );
        let model_line = concat!(
            r#"{"result":{"response":{"modelResponse":{"responseId":"assistant-1","message":"done","parentResponseId":"p1"}}}}"#,
            "\n",
        );

        let chunks: Vec<Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from(conv_line)),
            Ok(bytes::Bytes::from(user_line)),
            Ok(bytes::Bytes::from(token_line)),
            Ok(bytes::Bytes::from(model_line)),
        ];
        let byte_stream = Box::pin(stream::iter(chunks));
        let mut handle = StreamHandle::from_byte_stream(byte_stream, Duration::from_secs(5));

        let (conversation_id, response_id, parent_response_id) =
            handle.drain_ids(None).await.expect("drain ids");

        assert_eq!(conversation_id.as_str(), "c1");
        assert_eq!(response_id.as_str(), "assistant-1");
        assert_eq!(parent_response_id.expect("parent").as_str(), "p1");

        let next_event = handle
            .next_event()
            .await
            .expect("next event")
            .expect("some");
        let StreamEvent::ModelResponse(model_response) = next_event else {
            panic!("expected modelResponse after drain_ids");
        };
        assert_eq!(model_response.response_id.as_str(), "assistant-1");
    }

    #[tokio::test(start_paused = true)]
    async fn drain_ids_accepts_seeded_conversation_for_continuations() {
        use futures_util::stream;

        let user_line = concat!(
            r#"{"result":{"userResponse":{"responseId":"user-echo-cont","message":"hello","parentResponseId":"p1"}}}"#,
            "\n",
        );
        let token_line = concat!(
            r#"{"result":{"token":"working","messageTag":"summary","responseId":"assistant-cont","isThinking":true,"isSoftStop":false}}"#,
            "\n",
        );

        let chunks: Vec<Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from(user_line)),
            Ok(bytes::Bytes::from(token_line)),
        ];
        let byte_stream = Box::pin(stream::iter(chunks));
        let mut handle = StreamHandle::from_byte_stream(byte_stream, Duration::from_secs(5));

        let (conversation_id, response_id, parent_response_id) = handle
            .drain_ids(Some(ConversationId::new("c-seeded")))
            .await
            .expect("drain ids with seed");

        assert_eq!(conversation_id.as_str(), "c-seeded");
        assert_eq!(response_id.as_str(), "assistant-cont");
        assert_eq!(parent_response_id.expect("parent").as_str(), "p1");

        let next_event = handle.next_event().await.expect("next event");
        assert!(next_event.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn drain_ids_fails_for_continuation_without_conversation_frame() {
        use futures_util::stream;

        let user_line = concat!(
            r#"{"result":{"userResponse":{"responseId":"user-echo-only","message":"hello","parentResponseId":"p1"}}}"#,
            "\n",
        );
        let token_line = concat!(
            r#"{"result":{"token":"working","messageTag":"summary","responseId":"assistant-only","isThinking":true,"isSoftStop":false}}"#,
            "\n",
        );

        let chunks: Vec<Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from(user_line)),
            Ok(bytes::Bytes::from(token_line)),
        ];
        let byte_stream = Box::pin(stream::iter(chunks));
        let mut handle = StreamHandle::from_byte_stream(byte_stream, Duration::from_secs(5));

        let err = handle
            .drain_ids(None)
            .await
            .expect_err("missing conversation id");
        assert!(matches!(err, Error::StreamEnded));
    }

    #[test]
    fn stream_without_model_response_errors() {
        let conv = r#"{"result":{"conversation":{"conversationId":"c1","title":"t","starred":false,"createTime":"2026-04-17T00:00:00Z","modifyTime":"2026-04-17T00:00:00Z","systemPromptName":"","temporary":false}}}"#;
        let value = serde_json::from_str::<Value>(conv).expect("json");
        let event = parse_envelope(value).expect("parse");

        let mut collector = Collector::new(None);
        collector.ingest(event);
        let err = collector.finish().expect_err("no model response");
        assert!(matches!(err, Error::StreamEnded));
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_does_not_trip_when_chunks_arrive_in_time() {
        use futures_util::stream;

        let conv_line = concat!(
            r#"{"result":{"conversation":{"conversationId":"c1","title":"t","starred":false,"createTime":"2026-04-17T00:00:00Z","modifyTime":"2026-04-17T00:00:00Z","systemPromptName":"","temporary":false}}}"#,
            "\n",
        );
        let model_line = concat!(
            r#"{"result":{"response":{"modelResponse":{"responseId":"r1","message":"hello","sender":"ASSISTANT","parentResponseId":"p1"}}}}"#,
            "\n",
        );

        let chunks: Vec<Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from(conv_line)),
            Ok(bytes::Bytes::from(model_line)),
        ];
        let byte_stream = Box::pin(stream::iter(chunks));
        let handle = StreamHandle::from_byte_stream(byte_stream, Duration::from_secs(5));
        let result = handle.collect_final(None).await.expect("drain");
        assert_eq!(result.conversation_id.as_str(), "c1");
        assert_eq!(result.response_id.as_str(), "r1");
        assert_eq!(result.message, "hello");
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_fires_when_stream_stalls() {
        use std::pin::Pin;

        use futures_util::stream::{self, Stream, StreamExt};

        let conv_line = concat!(
            r#"{"result":{"conversation":{"conversationId":"c1","title":"t","starred":false,"createTime":"2026-04-17T00:00:00Z","modifyTime":"2026-04-17T00:00:00Z","systemPromptName":"","temporary":false}}}"#,
            "\n",
        );
        // First chunk arrives immediately, then a 60-second stall — well past
        // the 5-second idle budget. We wrap each item in a short delay so the
        // stream actually yields to the runtime between items.
        let first = async move { Ok::<_, Error>(bytes::Bytes::from(conv_line)) };
        let stalled = async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(bytes::Bytes::from_static(b"{}\n"))
        };
        let seq = stream::once(first).chain(stream::once(stalled));
        let byte_stream: Pin<Box<dyn Stream<Item = Result<bytes::Bytes>> + Send>> =
            Box::pin(seq.boxed());

        let mut handle = StreamHandle::from_byte_stream(byte_stream, Duration::from_secs(5));
        let first_event = handle.next_event().await.expect("first").expect("some");
        assert!(matches!(first_event, StreamEvent::Conversation(_)));

        let err = handle.next_event().await.expect_err("idle timeout");
        let Error::StreamIdleTimeout { timeout_seconds } = err else {
            panic!("expected StreamIdleTimeout, got {err:?}");
        };
        assert_eq!(timeout_seconds, 5);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_timeout_disabled_when_zero() {
        use futures_util::stream;

        let conv_line = concat!(
            r#"{"result":{"conversation":{"conversationId":"c1","title":"t","starred":false,"createTime":"2026-04-17T00:00:00Z","modifyTime":"2026-04-17T00:00:00Z","systemPromptName":"","temporary":false}}}"#,
            "\n",
        );
        let model_line = concat!(
            r#"{"result":{"response":{"modelResponse":{"responseId":"r1","message":"hello","sender":"ASSISTANT","parentResponseId":"p1"}}}}"#,
            "\n",
        );
        let chunks: Vec<Result<bytes::Bytes>> = vec![
            Ok(bytes::Bytes::from(conv_line)),
            Ok(bytes::Bytes::from(model_line)),
        ];
        let byte_stream = Box::pin(stream::iter(chunks));
        let handle = StreamHandle::from_byte_stream(byte_stream, Duration::ZERO);
        let result = handle.collect_final(None).await.expect("drain");
        assert_eq!(result.response_id.as_str(), "r1");
    }
}
