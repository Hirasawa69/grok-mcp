//! Typed error model for the grok-mcp library.
//!
//! Library-layer code returns [`Error`] / [`Result`]; the MCP tool-handler boundary
//! converts those into [`rmcp::ErrorData`] via [`Error::into_mcp`]. Application code
//! (`main.rs`) wraps the top-level bootstrap in `anyhow`.

use rmcp::ErrorData;
use serde_json::json;

use crate::models::api_error::GrokApiError;

/// Result alias carrying the library [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// Library error kinds. All fallible paths in `grok-mcp` produce one of these.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Config file or environment variable parsing / validation.
    #[error("config error: {0}")]
    Config(#[from] ConfigError),

    /// Cookie header is missing or malformed.
    #[error("cookie error: {0}")]
    Cookie(#[from] CookieError),

    /// grok.com rejected the request as unauthenticated. The caller should
    /// refresh cookies from their browser and retry.
    #[error("grok auth expired or cookies invalid")]
    AuthExpired,

    /// grok.com returned `403` with an upstream envelope (tier gating, etc.).
    #[error("grok forbidden: {0}")]
    Forbidden(GrokApiError),

    /// Any other 4xx response carrying the Grok `{code, message, details}` envelope.
    #[error("grok api error: {0}")]
    Api(GrokApiError),

    /// Non-JSON upstream error (Cloudflare challenge page, 5xx HTML, etc.).
    #[error("upstream status {status}: {body}")]
    UpstreamStatus {
        /// HTTP status code.
        status: u16,
        /// Short body preview for diagnostics (truncated to 2 KiB upstream).
        body: String,
    },

    /// An NDJSON stream frame failed to parse.
    #[error("stream decode error: {0}")]
    StreamDecode(String),

    /// The stream ended before a terminal `modelResponse` event was emitted.
    #[error("stream ended without final model response")]
    StreamEnded,

    /// The stream produced no bytes within the configured idle timeout.
    #[error("stream idle for {timeout_seconds}s; grok.com stopped sending data")]
    StreamIdleTimeout {
        /// Configured idle budget that was exceeded, in seconds.
        timeout_seconds: u64,
    },

    /// Transport-level HTTP failure (connection, timeout, TLS, …).
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),

    /// Local I/O failure (file read for uploads, config load, …).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Config-loading error detail.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Config file could not be read.
    #[error("failed to read config file {path}: {source}")]
    ReadFile {
        /// The offending path.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// TOML parse failure.
    #[error("failed to parse config toml: {0}")]
    Toml(#[from] toml::de::Error),
    /// Required field missing after layering file + env.
    #[error("missing required config value: {0}")]
    MissingRequired(&'static str),
    /// Env var value could not be parsed.
    #[error("invalid env var {name}: {value}")]
    InvalidEnv {
        /// Env var name.
        name: &'static str,
        /// Raw offending value.
        value: String,
    },
}

/// Cookie-header validation error.
#[derive(Debug, thiserror::Error)]
pub enum CookieError {
    /// The cookie header string was empty.
    #[error("cookie header is empty")]
    Empty,
    /// The cookie header could not be split into `name=value` pairs.
    #[error("cookie header is malformed")]
    Malformed,
    /// A required cookie name was not present.
    #[error("cookie is missing required name: {0}")]
    MissingRequired(&'static str),
}

impl Error {
    /// Convert a library error into the MCP wire-level [`ErrorData`].
    ///
    /// * `Cookie` / `AuthExpired` map to `invalid_params` — the user can fix the
    ///   input (refresh cookies).
    /// * `Forbidden` / `Api` map to `internal_error` but carry the upstream
    ///   `{code, message}` in the `data` field.
    /// * Everything else is `internal_error` with a short detail payload.
    pub fn into_mcp(self) -> ErrorData {
        match self {
            Self::Cookie(error) => ErrorData::invalid_params(
                format!("cookie error: {error}"),
                Some(
                    json!({ "hint": "refresh cookies from browser devtools (Application → Cookies)" }),
                ),
            ),
            Self::AuthExpired => ErrorData::invalid_params(
                "grok cookies are expired or invalid; refresh from browser devtools",
                None,
            ),
            Self::Forbidden(api_error) => ErrorData::internal_error(
                format!("grok forbidden: {}", api_error.message),
                Some(json!({
                    "grok_code": api_error.code,
                    "grok_message": api_error.message,
                    "details": api_error.details,
                })),
            ),
            Self::Api(api_error) => ErrorData::internal_error(
                format!("grok api error: {}", api_error.message),
                Some(json!({
                    "grok_code": api_error.code,
                    "grok_message": api_error.message,
                    "details": api_error.details,
                })),
            ),
            Self::UpstreamStatus { status, body } => ErrorData::internal_error(
                format!("upstream http {status}"),
                Some(json!({ "status": status, "body": body })),
            ),
            Self::StreamDecode(detail) => {
                ErrorData::internal_error(format!("stream decode: {detail}"), None)
            }
            Self::StreamEnded => ErrorData::internal_error(
                "grok stream ended without a terminal modelResponse",
                None,
            ),
            Self::StreamIdleTimeout { timeout_seconds } => ErrorData::internal_error(
                format!("grok stream idle for {timeout_seconds}s"),
                Some(json!({ "timeout_seconds": timeout_seconds })),
            ),
            Self::Transport(error) => {
                ErrorData::internal_error(format!("transport: {error}"), None)
            }
            Self::Io(error) => ErrorData::internal_error(format!("io: {error}"), None),
            Self::Serde(error) => ErrorData::internal_error(format!("serde: {error}"), None),
            Self::Config(error) => ErrorData::internal_error(format!("config: {error}"), None),
        }
    }
}

impl From<Error> for ErrorData {
    fn from(error: Error) -> Self {
        error.into_mcp()
    }
}
