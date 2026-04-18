//! Progress-notification forwarding for streaming chat tools.
//!
//! If the MCP caller supplied a `progressToken` in request metadata, we forward
//! `final`/`header`/`summary` token fragments plus selected discrete stream
//! events as [`ProgressNotificationParam`] messages. Fragments are batched and
//! flushed on a short timer / character budget so we don't hammer the transport.

use std::time::Duration;

use tokio::time::Instant;

use rmcp::{
    Peer, RoleServer,
    model::{ProgressNotificationParam, ProgressToken},
};

use crate::models::{chat::StreamEvent, common::MessageTag};

const FLUSH_INTERVAL: Duration = Duration::from_millis(80);
const FLUSH_CHARS: usize = 200;
pub(crate) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Runtime forwarder. Construct once per streaming tool invocation.
pub struct ProgressForwarder {
    token: Option<ProgressToken>,
    peer: Option<Peer<RoleServer>>,
    buffer: String,
    counter: u64,
    last_activity: Instant,
}

impl ProgressForwarder {
    /// Build a forwarder from the request metadata's progress token.
    ///
    /// If the caller didn't supply a token, this returns a no-op forwarder that
    /// still consumes events so the caller can keep a single code path.
    pub fn new(token: Option<ProgressToken>, peer: Peer<RoleServer>) -> Self {
        Self {
            token,
            peer: Some(peer),
            buffer: String::new(),
            counter: 0,
            last_activity: Instant::now(),
        }
    }

    #[cfg(test)]
    fn for_tests(token: Option<ProgressToken>) -> Self {
        Self {
            token,
            peer: None,
            buffer: String::new(),
            counter: 0,
            last_activity: Instant::now(),
        }
    }

    /// Feed one stream event to the forwarder.
    pub async fn observe(&mut self, event: &StreamEvent) {
        let Some(token) = self.token.clone() else {
            return;
        };
        match event {
            StreamEvent::Token {
                token: text,
                message_tag,
                ..
            } => {
                if !matches!(
                    message_tag.as_ref(),
                    Some(MessageTag::Final | MessageTag::Header | MessageTag::Summary)
                ) {
                    return;
                }
                self.buffer.push_str(text);
                if self.buffer.len() >= FLUSH_CHARS
                    || self.last_activity.elapsed() >= FLUSH_INTERVAL
                {
                    self.flush(token).await;
                }
            }
            StreamEvent::ProgressReport {
                category, state, ..
            } => {
                let message = format!("{category}: {state}");
                self.send(token, Some(message)).await;
            }
            StreamEvent::ToolUsageCard(card) => {
                self.flush(token.clone()).await;
                self.send(token, Some(format!("tool: {}", tool_label(card))))
                    .await;
            }
            StreamEvent::WebSearchResults { results, .. } => {
                self.flush(token.clone()).await;
                let message = format!("web search: {} results", results.len());
                self.send(token, Some(message)).await;
            }
            StreamEvent::UserResponse(_) => {
                self.flush(token.clone()).await;
                self.send(token, Some("received".to_owned())).await;
            }
            StreamEvent::ModelResponse(_) => {
                self.flush(token.clone()).await;
                self.send(token, Some("answering".to_owned())).await;
            }
            _ => (),
        }
    }

    /// Send a neutral liveness notification after a quiet interval.
    pub async fn heartbeat(&mut self) {
        let Some(token) = self.token.clone() else {
            return;
        };
        if !self.is_heartbeat_due() {
            return;
        }
        self.flush(token.clone()).await;
        self.send(token, Some("working...".to_owned())).await;
    }

    /// Flush any buffered tokens at the end of the stream.
    pub async fn finalize(mut self) {
        let Some(token) = self.token.clone() else {
            return;
        };
        if !self.buffer.is_empty() {
            self.flush(token).await;
        }
    }

    async fn flush(&mut self, token: ProgressToken) {
        if self.buffer.is_empty() {
            return;
        }
        let flushed = std::mem::take(&mut self.buffer);
        self.send(token, Some(flushed)).await;
    }

    async fn send(&mut self, token: ProgressToken, message: Option<String>) {
        self.counter += 1;
        self.last_activity = Instant::now();
        let Some(peer) = &self.peer else {
            return;
        };
        let param = ProgressNotificationParam {
            progress_token: token,
            progress: self.counter as f64,
            total: None,
            message,
        };
        if let Err(error) = peer.notify_progress(param).await {
            tracing::warn!(%error, "failed to send progress notification");
        }
    }

    fn is_heartbeat_due(&self) -> bool {
        self.last_activity.elapsed() >= HEARTBEAT_INTERVAL
    }
}

/// Extract the progress token from request metadata, if the client supplied one.
#[must_use]
pub fn extract_progress_token(meta: &rmcp::model::Meta) -> Option<ProgressToken> {
    meta.get_progress_token()
}

fn tool_label(card: &crate::models::common::ToolUsageCard) -> String {
    if let Some(name) = card.extra.get("toolName").and_then(|value| value.as_str()) {
        return name.to_owned();
    }

    card.extra
        .keys()
        .find(|key| *key != "toolUsageCardId")
        .cloned()
        .unwrap_or_else(|| "tool".to_owned())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::time::Instant;

    use super::{HEARTBEAT_INTERVAL, ProgressForwarder, tool_label};
    use crate::{
        models::{StreamEvent, ToolUsageCard, ToolUsageCardId},
        server::progress::FLUSH_INTERVAL,
    };
    use rmcp::model::{NumberOrString, ProgressToken};

    #[tokio::test(start_paused = true)]
    async fn heartbeat_is_noop_without_progress_token() {
        let mut forwarder = ProgressForwarder::for_tests(None);

        tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_secs(1)).await;
        forwarder.heartbeat().await;

        assert_eq!(forwarder.counter, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_waits_for_quiet_interval() {
        let mut forwarder =
            ProgressForwarder::for_tests(Some(ProgressToken(NumberOrString::String("p".into()))));

        tokio::time::advance(HEARTBEAT_INTERVAL - Duration::from_secs(1)).await;
        forwarder.heartbeat().await;
        assert_eq!(forwarder.counter, 0);

        tokio::time::advance(Duration::from_secs(2)).await;
        forwarder.heartbeat().await;
        assert_eq!(forwarder.counter, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn web_search_event_forwards_single_notification() {
        let mut forwarder =
            ProgressForwarder::for_tests(Some(ProgressToken(NumberOrString::String("p".into()))));
        forwarder.buffer.push_str("pending");
        forwarder.last_activity = Instant::now() - FLUSH_INTERVAL;

        let event = StreamEvent::WebSearchResults {
            response_id: None,
            tool_usage_card_id: None,
            message_tag: None,
            results: Vec::new(),
        };
        forwarder.observe(&event).await;

        assert_eq!(forwarder.counter, 2);
        assert!(forwarder.buffer.is_empty());
    }

    #[test]
    fn tool_label_prefers_tool_name_field() {
        let mut extra = serde_json::Map::new();
        extra.insert(
            "toolName".to_owned(),
            serde_json::Value::String("gmailSearch".to_owned()),
        );
        let card = ToolUsageCard {
            tool_usage_card_id: ToolUsageCardId::new("tool-card-1"),
            extra,
        };

        assert_eq!(tool_label(&card), "gmailSearch");
    }
}
