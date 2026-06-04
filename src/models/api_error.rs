//! `{code, message, details}` error payload used by grok.com on non-2xx responses.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// grok.com application-error payload.
///
/// Some endpoints return this directly; others wrap it under an `error` field.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct GrokApiError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub details: Vec<serde_json::Value>,
}

impl fmt::Display for GrokApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "grok[{}] {}", self.code, self.message)
    }
}

impl GrokApiError {
    /// Heuristic check for Grok's anti-bot rejection envelope.
    ///
    /// The currently observed chat-stream block is HTTP `403` with code `7`
    /// and message `Request rejected by anti-bot rules.`. Keep the message
    /// check loose because this API is undocumented and may change wording.
    #[must_use]
    pub fn looks_like_anti_bot(&self) -> bool {
        let lower = self.message.to_lowercase();
        self.code == 7 && (lower.contains("anti-bot") || lower.contains("bot"))
    }

    /// Heuristic check for auth-expiry shape.
    ///
    /// grok.com does not document a stable error code for expired sessions. The
    /// heuristic matches HTTP `401`/`403` with a message that hints at session
    /// or SSO failure. Callers pair this with the HTTP status to decide whether
    /// to map the result to [`crate::error::Error::AuthExpired`].
    #[must_use]
    pub fn looks_like_auth_failure(&self) -> bool {
        let lower = self.message.to_lowercase();
        lower.contains("auth")
            || lower.contains("unauthor")
            || lower.contains("session")
            || lower.contains("invalid_sso")
            || lower.contains("login")
    }
}
