//! MCP server surface: tool definitions, routing, and request handling.

pub mod output;
pub mod params;
pub mod progress;
pub mod schema_helpers;

use std::{path::Path, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rmcp::{
    ErrorData, Json, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Implementation, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use serde_json::json;
use tokio::time::MissedTickBehavior;

use crate::{
    client::{
        GrokClient,
        stream::{
            Collector, extract_citations_from_card_attachments, extract_web_search_results,
            merge_citations_from_message,
        },
    },
    config::RuntimeState,
    error::Error,
    models::{
        ChatOptions, IntegrationFlags,
        chat::{FinalChatResult, LoadedResponse},
        common::{
            AgentMessage, ConversationId, FollowUpSuggestion, Mode, ResponseId, ResponseStep,
            ToolUsageCard, UserId, WebSearchResult,
        },
        subscriptions::{SubscriptionStatus, Tier},
    },
    server::{
        output::{
            AuthStatus, DefaultsOutput, GetConversationOutput, ListConversationsOutput, PollOutput,
            PollStatus, RateLimitsOutput, ResearchStartOutput, StepThinking, UploadFileOutput,
        },
        params::{
            AskParams, GetConversationParams, ListConversationsParams, PollParams,
            RateLimitsParams, SetDefaultsParams, SkillsParams, UploadFileParams, Verbosity,
            resolve_verbosity,
        },
        progress::{ProgressForwarder, extract_progress_token},
    },
};

/// MCP server that bridges an in-process agent to grok.com.
///
/// Cloning is cheap — [`GrokClient`] and [`RuntimeState`] wrap their inner state
/// with `Arc`.
#[derive(Clone)]
pub struct Server {
    client: Arc<GrokClient>,
    runtime: RuntimeState,
    tool_router: ToolRouter<Self>,
}

impl Server {
    /// Wire a new server around an existing [`GrokClient`].
    #[must_use]
    pub fn new(client: GrokClient, runtime: RuntimeState) -> Self {
        Self {
            client: Arc::new(client),
            runtime,
            tool_router: Self::tool_router(),
        }
    }

    /// Return the full list of MCP tools this server exposes (name + description).
    ///
    /// The [`list_tools`](crate::Server::list_tools) subcommand renders this;
    /// keeping it on the public surface lets external callers document the
    /// server without constructing a transport.
    #[must_use]
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }
}

#[tool_router(router = tool_router)]
impl Server {
    /// `grok_check_auth` — smoke-test the session by hitting `/rest/subscriptions`.
    #[tool(
        name = "grok_check_auth",
        description = "Check that grok.com cookies still work. \
        Returns the logged-in user id, subscription tier, and status."
    )]
    pub async fn grok_check_auth(&self) -> Result<Json<AuthStatus>, ErrorData> {
        let subscriptions = self.client.subscriptions().await.map_err(Error::into_mcp)?;
        let (user_id, tier, status, active_until) = match subscriptions.subscriptions.first() {
            Some(sub) => (
                Some(sub.xai_user_id.clone()),
                Some(sub.tier.clone()),
                Some(sub.status.clone()),
                sub.stripe
                    .as_ref()
                    .and_then(|value| value.get("currentPeriodEnd"))
                    .and_then(|value| value.as_str())
                    .map(str::to_owned),
            ),
            None => (
                None::<UserId>,
                None::<Tier>,
                None::<SubscriptionStatus>,
                None,
            ),
        };
        Ok(Json(AuthStatus {
            authenticated: true,
            user_id,
            tier,
            status,
            active_until,
        }))
    }

    /// `grok_rate_limits` — query remaining/total queries for the given mode.
    #[tool(
        name = "grok_rate_limits",
        description = "Remaining query budget (per-mode rate limit). Defaults to the current runtime mode."
    )]
    pub async fn grok_rate_limits(
        &self,
        Parameters(params): Parameters<RateLimitsParams>,
    ) -> Result<Json<RateLimitsOutput>, ErrorData> {
        let defaults = self.runtime.defaults().await;
        let mode = params.mode.unwrap_or(defaults.mode);
        let limits = self
            .client
            .rate_limits(&mode)
            .await
            .map_err(Error::into_mcp)?;
        Ok(Json(limits.into()))
    }

    /// `grok_list_skills` — built-in skills catalog.
    #[tool(
        name = "grok_list_skills",
        description = "List grok.com built-in skills (catalog of tools Grok itself can invoke)."
    )]
    pub async fn grok_list_skills(
        &self,
        Parameters(params): Parameters<SkillsParams>,
    ) -> Result<Json<crate::models::SkillsResponse>, ErrorData> {
        let locale = params.locale.as_deref().unwrap_or("en");
        let response = self.client.skills(locale).await.map_err(Error::into_mcp)?;
        Ok(Json(response))
    }

    /// `grok_list_conversations` — paginated conversation list.
    #[tool(
        name = "grok_list_conversations",
        description = "List grok.com conversations. Pass page_token from a prior response to paginate."
    )]
    pub async fn grok_list_conversations(
        &self,
        Parameters(params): Parameters<ListConversationsParams>,
    ) -> Result<Json<ListConversationsOutput>, ErrorData> {
        let list_builder = self
            .client
            .conversations()
            .list()
            .maybe_page_size(params.page_size)
            .maybe_page_token(params.page_token);
        let list = list_builder.send().await.map_err(Error::into_mcp)?;
        Ok(Json(ListConversationsOutput {
            conversations: list.conversations,
            next_page_token: list.next_page_token,
        }))
    }

    /// `grok_get_conversation` — fetch typed conversation metadata + optional thread.
    #[tool(
        name = "grok_get_conversation",
        description = "Fetch a conversation. Set include_messages to also load the full message thread."
    )]
    pub async fn grok_get_conversation(
        &self,
        Parameters(params): Parameters<GetConversationParams>,
    ) -> Result<Json<GetConversationOutput>, ErrorData> {
        let output = self
            .client
            .conversations()
            .get(params.conversation_id.clone())
            .maybe_include_messages(Some(params.include_messages))
            .send()
            .await
            .map_err(Error::into_mcp)?;
        Ok(Json(output))
    }

    /// `grok_research` — hero tool. Streams a deep-research chat response.
    ///
    /// When the caller leaves `mode` unset, this tool defaults to
    /// [`Mode::Expert`] regardless of the runtime defaults — deep research is
    /// what the tool is for, and the runtime default is reserved for
    /// general-purpose tools (e.g. rate limits).
    #[tool(
        name = "grok_research",
        description = "Deep research via Grok in a single call. Streams the answer and \
        blocks until complete. Returns conversation_id, response_id, message, and \
        (by default) citations + web search + thinking. \
        \
        Use this ONLY for: (1) non-expert modes (auto/fast) where latency is \
        predictable, OR (2) expert queries you expect to complete within your \
        MCP client's tool-call timeout (typically 60-120s). \
        \
        For expert-mode heavy research, prefer grok_research_start + \
        grok_research_poll — if this call exceeds the client timeout, the answer \
        is LOST and cannot be recovered. \
        \
        Pass verbosity=\"minimal\" if you only need the answer; \"standard\" \
        (default) for web search + citations + thinking; \"full\" for per-step \
        reasoning (one extra HTTP round-trip)."
    )]
    pub async fn grok_research(
        &self,
        Parameters(params): Parameters<AskParams>,
        ctx: RequestContext<RoleServer>,
    ) -> Result<Json<FinalChatResult>, ErrorData> {
        let verbosity = resolve_verbosity(params.verbosity, params.full_details);
        let effective_mode = params.mode.clone().unwrap_or(Mode::Expert);
        let progress_token = extract_progress_token(&ctx.meta);
        let mut forwarder = ProgressForwarder::new(progress_token, ctx.peer.clone());

        let (mut stream, seed_conv_id) = self
            .open_research_stream(&params, effective_mode)
            .await
            .map_err(Error::into_mcp)?;

        let mut collector = Collector::new(seed_conv_id);
        let mut heartbeat_ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        heartbeat_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                event = stream.next_event() => {
                    let Some(event) = event.map_err(Error::into_mcp)? else {
                        break;
                    };
                    forwarder.observe(&event).await;
                    collector.ingest(event);
                }
                _ = heartbeat_ticker.tick() => {
                    forwarder.heartbeat().await;
                }
            }
        }
        forwarder.finalize().await;
        let mut result = collector.finish().map_err(Error::into_mcp)?;
        if verbosity == Verbosity::Full {
            let (steps, agent_messages) = self
                .hydrate_response_steps(&result)
                .await
                .map_err(Error::into_mcp)?;
            result.steps = steps;
            result.agent_messages = agent_messages;
        }
        apply_verbosity(&mut result, verbosity);
        Ok(Json(result))
    }

    #[tool(
        name = "grok_research_start",
        description = "Begin a long-running Grok research request without waiting for the \
        answer. Returns conversation_id + response_id immediately; use \
        grok_research_poll to retrieve the result. \
        \
        Prefer this over grok_research for any expert-mode query or when you \
        suspect the response will take > 60s. Safe against client tool-call \
        timeouts — the request continues on Grok's side and the result is \
        fetchable by response_id for hours afterward."
    )]
    pub async fn grok_research_start(
        &self,
        Parameters(params): Parameters<AskParams>,
    ) -> Result<Json<ResearchStartOutput>, ErrorData> {
        let effective_mode = params.mode.clone().unwrap_or(Mode::Expert);

        let seed_conversation_id = params.conversation_id.clone();
        let (mut stream, _) = self
            .open_research_stream(&params, effective_mode)
            .await
            .map_err(Error::into_mcp)?;

        let (conversation_id, response_id, parent_response_id) = stream
            .drain_ids(seed_conversation_id)
            .await
            .map_err(Error::into_mcp)?;

        Ok(Json(ResearchStartOutput {
            conversation_id,
            response_id,
            parent_response_id,
        }))
    }

    #[tool(
        name = "grok_research_poll",
        description = "Poll for a previously started research result by \
        (conversation_id, response_id). Returns status='in_progress' if Grok is \
        still generating, 'completed' with the full result when ready, or \
        'not_found' for an unknown id pair. \
        \
        Pass verbosity=\"minimal\"/\"standard\"/\"full\" to control \
        response size. include_thinking=true adds per-step reasoning traces \
        separately from the verbosity=\"full\" hydrated fields."
    )]
    pub async fn grok_research_poll(
        &self,
        Parameters(params): Parameters<PollParams>,
    ) -> Result<Json<PollOutput>, ErrorData> {
        let verbosity = resolve_verbosity(params.verbosity, params.full_details);
        let response_id = ResponseId::new(params.response_id);
        let loaded = self
            .client
            .conversations()
            .load_responses(&params.conversation_id, std::slice::from_ref(&response_id))
            .await
            .map_err(Error::into_mcp)?;
        let Some(loaded_response) = loaded
            .responses
            .into_iter()
            .find(|response| response.response_id == response_id)
        else {
            return Ok(Json(PollOutput {
                status: PollStatus::NotFound,
                result: None,
                thinking_steps: None,
            }));
        };

        if !response_is_ready(&loaded_response) {
            return Ok(Json(PollOutput {
                status: PollStatus::InProgress,
                result: None,
                thinking_steps: None,
            }));
        }

        let conversation_id = ConversationId::new(params.conversation_id);
        let mut result = build_result_from_loaded_response(conversation_id, &loaded_response)
            .map_err(Error::into_mcp)?;
        let thinking_steps = if params.include_thinking {
            Some(extract_step_thinking(&loaded_response.steps))
        } else {
            None
        };
        if verbosity == Verbosity::Full {
            let agent_messages = extract_agent_messages(&loaded_response.steps);
            result.agent_messages = agent_messages;
            result.steps = Some(loaded_response.steps.clone());
        }
        apply_verbosity(&mut result, verbosity);

        Ok(Json(PollOutput {
            status: PollStatus::Completed,
            result: Some(Box::new(result)),
            thinking_steps,
        }))
    }

    /// `grok_upload_file` — JSON+base64 upload.
    #[tool(
        name = "grok_upload_file",
        description = "Upload a file and get back an attachment id to pass into grok_research. \
        Provide content_base64 or local_path."
    )]
    pub async fn grok_upload_file(
        &self,
        Parameters(params): Parameters<UploadFileParams>,
    ) -> Result<Json<UploadFileOutput>, ErrorData> {
        let bytes = load_upload_bytes(&params).await?;
        let mime = params
            .mime_type
            .clone()
            .or_else(|| guess_mime_from_name(&params.file_name))
            .unwrap_or_else(|| "application/octet-stream".to_owned());
        let response = self
            .client
            .uploads()
            .upload()
            .file_name(params.file_name)
            .mime_type(mime)
            .content_base64(BASE64_STANDARD.encode(bytes))
            .send()
            .await
            .map_err(Error::into_mcp)?;
        Ok(Json(UploadFileOutput {
            file_metadata_id: response.file_metadata_id,
            file_uri: response.file_uri,
            mime_type: response.file_mime_type,
            file_name: response.file_name,
            create_time: response.create_time,
        }))
    }

    /// `grok_get_defaults` — current runtime defaults.
    #[tool(
        name = "grok_get_defaults",
        description = "Read the current runtime defaults (mode + flags)."
    )]
    pub async fn grok_get_defaults(&self) -> Result<Json<DefaultsOutput>, ErrorData> {
        let defaults = self.runtime.defaults().await;
        Ok(Json(DefaultsOutput { defaults }))
    }

    /// `grok_set_defaults` — mutate runtime defaults.
    #[tool(
        name = "grok_set_defaults",
        description = "Override runtime defaults in-memory (not persisted). \
        Only fields you pass are changed; returns the full post-update snapshot."
    )]
    pub async fn grok_set_defaults(
        &self,
        Parameters(params): Parameters<SetDefaultsParams>,
    ) -> Result<Json<DefaultsOutput>, ErrorData> {
        let mut current = self.runtime.defaults().await;
        if let Some(mode) = params.mode {
            current.mode = mode;
        }
        if let Some(flag) = params.disable_search {
            current.disable_search = flag;
        }
        if let Some(flag) = params.force_concise {
            current.force_concise = flag;
        }
        if let Some(flag) = params.disable_memory {
            current.disable_memory = flag;
        }
        if let Some(flag) = params.enable_image_generation {
            current.enable_image_generation = flag;
        }
        if let Some(count) = params.image_generation_count {
            current.image_generation_count = count;
        }
        if let Some(flag) = params.enable_side_by_side {
            current.enable_side_by_side = flag;
        }
        if let Some(flag) = params.disable_text_follow_ups {
            current.disable_text_follow_ups = flag;
        }
        let snapshot = self.runtime.replace(current).await;
        Ok(Json(DefaultsOutput { defaults: snapshot }))
    }
}

impl Server {
    async fn open_research_stream(
        &self,
        params: &AskParams,
        effective_mode: Mode,
    ) -> crate::error::Result<(crate::client::stream::StreamHandle, Option<ConversationId>)> {
        let mut options = chat_options_from_params(params);
        options.mode = Some(effective_mode);

        match params.conversation_id.clone() {
            Some(conversation_id) => {
                let handle = self
                    .client
                    .conversations()
                    .continue_(conversation_id.clone())
                    .message(params.message.clone())
                    .maybe_parent_response_id(params.parent_response_id.clone())
                    .options(options)
                    .send()
                    .await?;
                Ok((handle, Some(conversation_id)))
            }
            None => {
                let handle = self
                    .client
                    .conversations()
                    .start(params.message.clone())
                    .options(options)
                    .send()
                    .await?;
                Ok((handle, None))
            }
        }
    }

    async fn hydrate_response_steps(
        &self,
        result: &FinalChatResult,
    ) -> crate::error::Result<(Option<Vec<ResponseStep>>, Vec<AgentMessage>)> {
        let conversation_id = result.conversation_id.as_str();
        let response_id = &result.response_id;
        let node_result = self
            .client
            .conversations()
            .response_node(conversation_id, true)
            .await?;
        let has_response = node_result
            .response_nodes
            .iter()
            .any(|node| &node.response_id == response_id);
        if !has_response {
            return Ok((None, Vec::new()));
        }

        let loaded = self
            .client
            .conversations()
            .load_responses(conversation_id, std::slice::from_ref(response_id))
            .await?;
        let steps = loaded
            .responses
            .into_iter()
            .find(|response| response.response_id == *response_id)
            .map(|response| response.steps);
        let agent_messages = steps
            .as_ref()
            .map(|steps| extract_agent_messages(steps))
            .unwrap_or_default();

        Ok((steps, agent_messages))
    }
}

fn apply_verbosity(result: &mut FinalChatResult, verbosity: Verbosity) {
    match verbosity {
        Verbosity::Minimal => {
            result.thinking = None;
            result.tool_usage_cards.clear();
            result.web_search_results.clear();
            result.follow_up_suggestions.clear();
            result.title = None;
            result.unknown_events.clear();
            result.steps = None;
            result.agent_messages.clear();
        }
        Verbosity::Standard => {
            result.steps = None;
            result.agent_messages.clear();
        }
        Verbosity::Full => (),
    }
}

fn build_result_from_loaded_response(
    conversation_id: ConversationId,
    loaded: &LoadedResponse,
) -> crate::error::Result<FinalChatResult> {
    let message = loaded
        .extra
        .get("message")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    let parent_response_id = loaded
        .extra
        .get("parentResponseId")
        .and_then(|value| value.as_str())
        .map(ResponseId::new);
    let title = loaded
        .extra
        .get("title")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let follow_up_suggestions = loaded
        .extra
        .get("followUpSuggestions")
        .cloned()
        .map(serde_json::from_value::<Vec<FollowUpSuggestion>>)
        .transpose()
        .map_err(crate::error::Error::Serde)?
        .unwrap_or_default();
    let web_search_results = match loaded.extra.get("webSearchResults") {
        Some(value) => extract_web_search_results(value.clone())?,
        None => loaded
            .steps
            .iter()
            .flat_map(|step| step.web_search_results.iter().cloned())
            .collect::<Vec<WebSearchResult>>(),
    };
    let card_attachments = loaded
        .extra
        .get("cardAttachmentsJson")
        .map(extract_citations_from_card_attachments)
        .unwrap_or_default();
    let citations = merge_citations_from_message(&message, &card_attachments);
    let tool_usage_cards = loaded
        .steps
        .iter()
        .flat_map(|step| step.tool_usage_cards.iter().cloned())
        .fold(Vec::<ToolUsageCard>::new(), |mut cards, card| {
            if !cards
                .iter()
                .any(|existing| existing.tool_usage_card_id == card.tool_usage_card_id)
            {
                cards.push(card);
            }
            cards
        });

    Ok(FinalChatResult {
        conversation_id,
        response_id: loaded.response_id.clone(),
        parent_response_id,
        message,
        thinking: None,
        citations,
        tool_usage_cards,
        web_search_results,
        follow_up_suggestions,
        title,
        steps: None,
        agent_messages: Vec::new(),
        unknown_events: Vec::new(),
    })
}

fn response_is_ready(loaded: &LoadedResponse) -> bool {
    // `partial: true` definitively blocks readiness — Grok is still streaming.
    // `partial: false` is NOT a reliable "ready" signal: grok.com stamps it
    // on the response record at creation time, before generation starts, so
    // we also require actual content (non-empty `message` or at least one
    // step). Missing `partial` field is treated the same as `false`.
    let partial_true = loaded
        .extra
        .get("partial")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    if partial_true {
        return false;
    }

    let has_message = loaded
        .extra
        .get("message")
        .and_then(|value| value.as_str())
        .is_some_and(|text| !text.is_empty());
    has_message || !loaded.steps.is_empty()
}

fn extract_agent_messages(steps: &[ResponseStep]) -> Vec<AgentMessage> {
    steps
        .iter()
        .flat_map(|step| {
            step.tool_usage_cards.iter().filter_map(|card| {
                let from = step.rollout_id.clone()?;
                let chatroom_send = card.extra.get("chatroomSend")?.as_object()?;
                let payload = chatroom_send
                    .get("args")
                    .and_then(|value| value.as_object())
                    .unwrap_or(chatroom_send);
                let to = payload.get("to")?.as_str()?.to_owned();
                let text = payload.get("message")?.as_str()?.to_owned();

                Some(AgentMessage { from, to, text })
            })
        })
        .collect::<Vec<AgentMessage>>()
}

fn extract_step_thinking(steps: &[ResponseStep]) -> Vec<StepThinking> {
    steps
        .iter()
        .map(|step| StepThinking {
            rollout_id: step.rollout_id.clone(),
            tags: step.tags.clone(),
            text: step.text.join("\n"),
        })
        .collect::<Vec<StepThinking>>()
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerInfo {
        // No `with_protocol_version` call: `ServerInfo::new` defaults to the
        // LATEST protocol version (2025-11-25 on pinned rmcp). Recent MCP
        // clients (opencode 1.4.7+, current Claude Desktop) require this;
        // rmcp negotiates downward for older clients.
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("grok-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Bridges a local agent to grok.com via browser-cookie auth. The hero tool is grok_research. Check auth with grok_check_auth before long runs.",
            )
    }
}

async fn load_upload_bytes(params: &UploadFileParams) -> Result<Vec<u8>, ErrorData> {
    match (&params.content_base64, &params.local_path) {
        (Some(content), None) => BASE64_STANDARD.decode(content.as_bytes()).map_err(|error| {
            ErrorData::invalid_params(format!("content_base64 is not valid base64: {error}"), None)
        }),
        (None, Some(path)) => tokio::fs::read(Path::new(path)).await.map_err(|error| {
            ErrorData::invalid_params(
                format!("failed to read local_path {path}: {error}"),
                Some(json!({ "path": path })),
            )
        }),
        (Some(_), Some(_)) => Err(ErrorData::invalid_params(
            "specify exactly one of content_base64 or local_path",
            None,
        )),
        (None, None) => Err(ErrorData::invalid_params(
            "one of content_base64 or local_path must be provided",
            None,
        )),
    }
}

fn guess_mime_from_name(name: &str) -> Option<String> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "txt" | "md" | "markdown" => "text/plain",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "csv" => "text/csv",
        "py" => "text/x-python",
        "rs" => "text/rust",
        _ => return None,
    };
    Some(mime.to_owned())
}

fn chat_options_from_params(params: &AskParams) -> ChatOptions {
    ChatOptions {
        mode: params.mode.clone(),
        attachments: params.attachments.clone(),
        disable_search: params.disable_search,
        force_concise: params.force_concise,
        disable_memory: params.disable_memory,
        integrations: IntegrationFlags {
            gmail: params.enable_gmail_search,
            google_calendar: params.enable_google_calendar_search,
            outlook: params.enable_outlook_search,
            outlook_calendar: params.enable_outlook_calendar_search,
            google_drive: params.enable_google_drive_search,
        },
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        GetConversationOutput, PollOutput, apply_verbosity, build_result_from_loaded_response,
        extract_agent_messages, extract_step_thinking, response_is_ready,
    };
    use crate::{
        models::{
            AgentMessage, Citation, ConversationId, FinalChatResult, FollowUpSuggestion,
            LoadedResponse, ResponseStep, RolloutId, ToolUsageCard, ToolUsageCardId,
            WebSearchResult,
        },
        server::{
            output::{PollStatus, StepThinking},
            params::{Verbosity, resolve_verbosity},
        },
    };

    fn populated_result() -> FinalChatResult {
        FinalChatResult {
            conversation_id: ConversationId::new("c1"),
            response_id: crate::models::ResponseId::new("r1"),
            parent_response_id: Some(crate::models::ResponseId::new("p1")),
            message: "done".to_owned(),
            thinking: Some("reasoning".to_owned()),
            citations: vec![Citation {
                card_id: "card-1".to_owned(),
                citation_id: Some("7".to_owned()),
                card_type: Some("citation_card".to_owned()),
                url: Some("https://example.com".to_owned()),
            }],
            tool_usage_cards: vec![ToolUsageCard {
                tool_usage_card_id: ToolUsageCardId::new("tool-card-1"),
                extra: serde_json::from_value(json!({ "kind": "web_search" }))
                    .expect("tool usage card extra"),
            }],
            web_search_results: vec![WebSearchResult {
                url: "https://example.com".to_owned(),
                title: "Example".to_owned(),
                preview: "preview".to_owned(),
                extra: serde_json::Map::new(),
            }],
            follow_up_suggestions: vec![FollowUpSuggestion {
                label: "next".to_owned(),
                properties: serde_json::Map::new(),
                tool_overrides: None,
                extra: serde_json::Map::new(),
            }],
            title: Some("title".to_owned()),
            steps: Some(vec![ResponseStep {
                text: vec!["step text".to_owned()],
                tags: vec!["analysis".to_owned()],
                rollout_id: Some(RolloutId::new("Agent 1")),
                message_step_id: Some(1),
                web_search_results: Vec::new(),
                tool_usage_cards: Vec::new(),
                tool_usage_results: Vec::new(),
                extra: serde_json::Map::new(),
            }]),
            agent_messages: vec![AgentMessage {
                from: RolloutId::new("Agent 1"),
                to: "Grok".to_owned(),
                text: "done".to_owned(),
            }],
            unknown_events: vec![
                serde_json::from_value(json!({ "frame": "unknown" })).expect("unknown event"),
            ],
        }
    }

    #[test]
    fn apply_verbosity_minimal_strips_everything_but_core() {
        let mut result = populated_result();

        apply_verbosity(&mut result, Verbosity::Minimal);

        assert_eq!(result.conversation_id.as_str(), "c1");
        assert_eq!(result.response_id.as_str(), "r1");
        assert_eq!(
            result.parent_response_id.as_ref().map(|id| id.as_str()),
            Some("p1")
        );
        assert_eq!(result.message, "done");
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.thinking, None);
        assert!(result.tool_usage_cards.is_empty());
        assert!(result.web_search_results.is_empty());
        assert!(result.follow_up_suggestions.is_empty());
        assert_eq!(result.title, None);
        assert!(result.steps.is_none());
        assert!(result.agent_messages.is_empty());
        assert!(result.unknown_events.is_empty());
    }

    #[test]
    fn apply_verbosity_standard_preserves_streamed_fields_drops_hydrated() {
        let mut result = populated_result();

        apply_verbosity(&mut result, Verbosity::Standard);

        assert_eq!(result.thinking.as_deref(), Some("reasoning"));
        assert_eq!(result.tool_usage_cards.len(), 1);
        assert_eq!(result.web_search_results.len(), 1);
        assert_eq!(result.follow_up_suggestions.len(), 1);
        assert_eq!(result.title.as_deref(), Some("title"));
        assert!(result.steps.is_none());
        assert!(result.agent_messages.is_empty());
        assert_eq!(result.unknown_events.len(), 1);
    }

    #[test]
    fn apply_verbosity_full_is_identity() {
        let original = populated_result();
        let mut result = original.clone();

        apply_verbosity(&mut result, Verbosity::Full);

        assert_eq!(
            serde_json::to_value(&result).expect("serialize full result"),
            serde_json::to_value(&original).expect("serialize original result")
        );
    }

    #[test]
    fn resolve_verbosity_prefers_explicit_over_full_details() {
        assert_eq!(resolve_verbosity(None, false), Verbosity::Standard);
        assert_eq!(resolve_verbosity(None, true), Verbosity::Full);
        assert_eq!(
            resolve_verbosity(Some(Verbosity::Minimal), true),
            Verbosity::Minimal
        );
    }

    #[test]
    fn full_details_alias_keeps_hydrated_poll_fields_when_verbosity_is_unset() {
        let loaded = LoadedResponse {
            response_id: crate::models::ResponseId::new("r1"),
            steps: vec![ResponseStep {
                text: vec!["done".to_owned()],
                tags: vec!["final".to_owned()],
                rollout_id: Some(RolloutId::new("Agent 1")),
                message_step_id: Some(1),
                web_search_results: Vec::new(),
                tool_usage_cards: vec![
                    serde_json::from_value::<ToolUsageCard>(json!({
                        "toolUsageCardId": "tool-card-1",
                        "chatroomSend": {
                            "args": {
                                "to": "Grok",
                                "message": "done"
                            }
                        }
                    }))
                    .expect("tool card"),
                ],
                tool_usage_results: Vec::new(),
                extra: serde_json::Map::new(),
            }],
            extra: serde_json::from_value(json!({ "message": "done", "partial": false }))
                .expect("extra map"),
        };
        let mut result = build_result_from_loaded_response(ConversationId::new("c1"), &loaded)
            .expect("result from loaded response");
        let verbosity = resolve_verbosity(None, true);

        if verbosity == Verbosity::Full {
            result.agent_messages = extract_agent_messages(&loaded.steps);
            result.steps = Some(loaded.steps.clone());
        }
        apply_verbosity(&mut result, verbosity);

        assert_eq!(result.message, "done");
        assert_eq!(result.steps.as_ref().map(Vec::len), Some(1));
        assert_eq!(result.agent_messages.len(), 1);
        assert_eq!(result.agent_messages[0].from.as_str(), "Agent 1");
    }

    #[test]
    fn extract_agent_messages_reads_chatroom_send_cards() {
        let step = ResponseStep {
            text: vec!["done".to_owned()],
            tags: vec!["final".to_owned()],
            rollout_id: Some(RolloutId::new("Agent 1")),
            message_step_id: None,
            web_search_results: Vec::new(),
            tool_usage_cards: vec![
                serde_json::from_value::<ToolUsageCard>(json!({
                    "toolUsageCardId": "tool-card-1",
                    "chatroomSend": {
                        "args": {
                            "to": "Grok",
                            "message": "Search complete"
                        }
                    }
                }))
                .expect("tool card"),
            ],
            tool_usage_results: Vec::new(),
            extra: serde_json::Map::new(),
        };

        let messages = extract_agent_messages(&[step]);

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].from.as_str(), "Agent 1");
        assert_eq!(messages[0].to, "Grok");
        assert_eq!(messages[0].text, "Search complete");
    }

    #[test]
    fn build_result_from_loaded_response_populates_citations_cards_and_agent_messages() {
        let loaded = LoadedResponse {
            response_id: crate::models::ResponseId::new("r1"),
            steps: vec![ResponseStep {
                text: vec!["done".to_owned()],
                tags: vec!["final".to_owned()],
                rollout_id: Some(RolloutId::new("Agent 1")),
                message_step_id: None,
                web_search_results: Vec::new(),
                tool_usage_cards: vec![
                    serde_json::from_value::<ToolUsageCard>(json!({
                        "toolUsageCardId": "tool-card-1",
                        "chatroomSend": {
                            "args": {
                                "to": "Grok",
                                "message": "done"
                            }
                        }
                    }))
                    .expect("tool card"),
                ],
                tool_usage_results: Vec::new(),
                extra: serde_json::Map::new(),
            }],
            extra: serde_json::from_value(json!({
                "message": concat!(
                    "Answer <grok:render card_id=\"card-1\" card_type=\"citation_card\" type=\"render_inline_citation\">",
                    "<argument name=\"citation_id\">5</argument></grok:render>"
                ),
                "partial": false,
                "parentResponseId": "p1",
                "title": "async title",
                "cardAttachmentsJson": [
                    "{\"id\":\"card-1\",\"cardType\":\"citation_card\",\"url\":\"https://example.com\"}"
                ]
            }))
            .expect("extra map"),
        };

        let mut result = build_result_from_loaded_response(ConversationId::new("c1"), &loaded)
            .expect("result from loaded response");
        result.agent_messages = extract_agent_messages(&loaded.steps);

        assert_eq!(result.conversation_id.as_str(), "c1");
        assert_eq!(result.response_id.as_str(), "r1");
        assert_eq!(result.parent_response_id.expect("parent").as_str(), "p1");
        assert_eq!(result.thinking, None);
        assert_eq!(result.title.as_deref(), Some("async title"));
        assert_eq!(result.citations.len(), 1);
        assert_eq!(result.citations[0].card_id, "card-1");
        assert_eq!(result.citations[0].citation_id.as_deref(), Some("5"));
        assert_eq!(
            result.citations[0].url.as_deref(),
            Some("https://example.com")
        );
        assert_eq!(result.tool_usage_cards.len(), 1);
        assert_eq!(result.agent_messages.len(), 1);
        assert_eq!(result.agent_messages[0].from.as_str(), "Agent 1");
    }

    #[test]
    fn poll_output_serializes_with_expected_status_shapes() {
        let in_progress = serde_json::to_value(PollOutput {
            status: PollStatus::InProgress,
            result: None,
            thinking_steps: None,
        })
        .expect("serialize in progress");
        assert_eq!(
            in_progress.get("status").and_then(|value| value.as_str()),
            Some("in_progress")
        );
        assert_eq!(in_progress.get("result"), None);

        let completed = serde_json::to_value(PollOutput {
            status: PollStatus::Completed,
            result: Some(Box::new(FinalChatResult {
                conversation_id: ConversationId::new("c1"),
                response_id: crate::models::ResponseId::new("r1"),
                parent_response_id: None,
                message: "done".to_owned(),
                thinking: None,
                citations: Vec::new(),
                tool_usage_cards: Vec::new(),
                web_search_results: Vec::new(),
                follow_up_suggestions: Vec::new(),
                title: None,
                steps: None,
                agent_messages: Vec::new(),
                unknown_events: Vec::new(),
            })),
            thinking_steps: Some(vec![StepThinking {
                rollout_id: Some(RolloutId::new("Agent 1")),
                tags: vec!["analysis".to_owned()],
                text: "reasoning".to_owned(),
            }]),
        })
        .expect("serialize completed");
        assert_eq!(
            completed.get("status").and_then(|value| value.as_str()),
            Some("completed")
        );
        assert!(completed.get("result").is_some());
        assert!(completed.get("thinking_steps").is_some());
    }

    fn loaded_response(extra: serde_json::Value, steps: Vec<ResponseStep>) -> LoadedResponse {
        LoadedResponse {
            response_id: crate::models::ResponseId::new("r1"),
            steps,
            extra: serde_json::from_value(extra).expect("extra map"),
        }
    }

    fn draft_step() -> ResponseStep {
        ResponseStep {
            text: vec!["draft".to_owned()],
            tags: Vec::new(),
            rollout_id: None,
            message_step_id: None,
            web_search_results: Vec::new(),
            tool_usage_cards: Vec::new(),
            tool_usage_results: Vec::new(),
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn response_is_ready_false_when_partial_true_even_with_message() {
        let loaded = loaded_response(json!({ "partial": true, "message": "half" }), Vec::new());
        assert!(!response_is_ready(&loaded));
    }

    #[test]
    fn response_is_ready_false_when_partial_false_but_no_content() {
        // Observed on the wire: grok.com writes `partial: false` the moment a
        // response row is created, before any content exists. The poll must
        // stay InProgress in this state.
        let loaded = loaded_response(json!({ "partial": false }), Vec::new());
        assert!(!response_is_ready(&loaded));
    }

    #[test]
    fn response_is_ready_false_when_partial_missing_and_no_content() {
        let loaded = loaded_response(json!({}), Vec::new());
        assert!(!response_is_ready(&loaded));
    }

    #[test]
    fn response_is_ready_false_when_message_empty() {
        let loaded = loaded_response(json!({ "partial": false, "message": "" }), Vec::new());
        assert!(!response_is_ready(&loaded));
    }

    #[test]
    fn response_is_ready_true_when_message_present() {
        let loaded = loaded_response(json!({ "partial": false, "message": "done" }), Vec::new());
        assert!(response_is_ready(&loaded));
    }

    #[test]
    fn response_is_ready_true_when_steps_present_even_with_empty_message() {
        let loaded = loaded_response(json!({ "partial": false }), vec![draft_step()]);
        assert!(response_is_ready(&loaded));
    }

    #[test]
    fn response_is_ready_false_when_partial_true_blocks_despite_steps() {
        // `partial: true` takes precedence — if Grok is still streaming,
        // don't report ready even when partial steps have arrived.
        let loaded = loaded_response(json!({ "partial": true }), vec![draft_step()]);
        assert!(!response_is_ready(&loaded));
    }

    #[test]
    fn extract_step_thinking_joins_text_fragments_with_newlines() {
        let steps = vec![ResponseStep {
            text: vec!["line one".to_owned(), "line two".to_owned()],
            tags: vec!["analysis".to_owned()],
            rollout_id: Some(RolloutId::new("Agent 2")),
            message_step_id: None,
            web_search_results: Vec::new(),
            tool_usage_cards: Vec::new(),
            tool_usage_results: Vec::new(),
            extra: serde_json::Map::new(),
        }];

        let thinking = extract_step_thinking(&steps);

        assert_eq!(thinking.len(), 1);
        assert_eq!(
            thinking[0].rollout_id.as_ref().expect("rollout").as_str(),
            "Agent 2"
        );
        assert_eq!(thinking[0].tags, vec!["analysis"]);
        assert_eq!(thinking[0].text, "line one\nline two");
    }

    #[test]
    fn get_conversation_output_serializes_summary_fields() {
        let output = GetConversationOutput {
            conversation: serde_json::from_value(json!({
                "conversationId": "c1",
                "title": "t",
                "starred": false,
                "createTime": "2026-04-18T00:00:00Z",
                "modifyTime": "2026-04-18T00:00:00Z",
                "systemPromptName": "",
                "temporary": false
            }))
            .expect("conversation"),
            response_nodes: None,
            responses: None,
            message_count: 2,
            total_chars: 7,
        };

        let value = serde_json::to_value(output).expect("serialize output");

        assert_eq!(
            value.get("message_count").and_then(|value| value.as_u64()),
            Some(2)
        );
        assert_eq!(
            value.get("total_chars").and_then(|value| value.as_u64()),
            Some(7)
        );
    }

    #[test]
    fn get_conversation_output_omits_optional_thread_fields_when_none() {
        let output = GetConversationOutput {
            conversation: serde_json::from_value(json!({
                "conversationId": "c1",
                "title": "t",
                "starred": false,
                "createTime": "2026-04-18T00:00:00Z",
                "modifyTime": "2026-04-18T00:00:00Z",
                "systemPromptName": "",
                "temporary": false
            }))
            .expect("conversation"),
            response_nodes: None,
            responses: None,
            message_count: 0,
            total_chars: 0,
        };

        let value = serde_json::to_value(output).expect("serialize output");

        assert_eq!(value.get("response_nodes"), None);
        assert_eq!(value.get("responses"), None);
        assert_eq!(
            value.get("message_count").and_then(|value| value.as_u64()),
            Some(0)
        );
        assert_eq!(
            value.get("total_chars").and_then(|value| value.as_u64()),
            Some(0)
        );
    }

    #[test]
    fn get_conversation_summary_counts_only_response_messages() {
        let loaded_responses = [
            LoadedResponse {
                response_id: crate::models::ResponseId::new("r1"),
                steps: Vec::new(),
                extra: serde_json::from_value(json!({
                    "message": "hello",
                    "metadata": { "ignored": true },
                    "title": "not counted"
                }))
                .expect("extra map"),
            },
            LoadedResponse {
                response_id: crate::models::ResponseId::new("r2"),
                steps: Vec::new(),
                extra: serde_json::from_value(json!({
                    "message": "世界",
                    "otherText": "ignored"
                }))
                .expect("extra map"),
            },
            LoadedResponse {
                response_id: crate::models::ResponseId::new("r3"),
                steps: Vec::new(),
                extra: serde_json::from_value(json!({
                    "metadata": { "message": "ignored nested" }
                }))
                .expect("extra map"),
            },
        ];

        let message_count = u32::try_from(loaded_responses.len()).expect("count fits u32");
        let total_chars =
            crate::client::conversations::count_response_message_chars(&loaded_responses);

        assert_eq!(message_count, 3);
        assert_eq!(total_chars, 7);
    }
}
