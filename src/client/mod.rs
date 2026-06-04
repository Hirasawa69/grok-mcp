//! HTTP client for the grok.com REST + NDJSON surface.
//!
//! The [`GrokClient`] owns a [`reqwest::Client`] plus the base URL, User-Agent,
//! cookie header, and a handle to [`crate::config::RuntimeState`] for
//! effective-defaults lookups. Simple direct methods live here; resource
//! builders live under [`conversations`] and [`uploads`]. The streaming parser
//! lives on [`stream`].

pub mod conversations;
pub mod headers;
pub mod statsig;
pub mod stream;
pub mod uploads;

use std::{sync::Arc, time::Duration};

use reqwest::{Response, StatusCode, cookie::Jar};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use tokio::sync::OnceCell;
use url::Url;

use crate::{
    client::statsig::ChallengeConfig,
    config::{NetworkConfig, RuntimeState},
    cookie::GrokCookie,
    error::{Error, Result},
    models::{
        AssetMetadata, GrokApiError, Mode, RateLimits, SkillsResponse, SubscriptionsResponse,
    },
};

/// Thin wrapper around [`reqwest::Client`] with grok.com-specific wiring.
///
/// Clone is cheap — the inner `reqwest::Client`, session warmup state, and
/// [`RuntimeState`] all share `Arc`-backed state.
#[derive(Clone)]
pub struct GrokClient {
    http: reqwest::Client,
    network: Arc<NetworkConfig>,
    challenge: Arc<ChallengeConfig>,
    session_ready: Arc<OnceCell<()>>,
    runtime: RuntimeState,
}

impl GrokClient {
    /// Build a new client with a pre-validated cookie and network config.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Transport`] if the underlying
    /// HTTP client fails to build.
    pub fn new(cookie: GrokCookie, network: NetworkConfig, runtime: RuntimeState) -> Result<Self> {
        Self::with_challenge(cookie, network, ChallengeConfig::default(), runtime)
    }

    /// Build a new client with an explicit anti-bot [`ChallengeConfig`].
    ///
    /// Use this to override the built-in challenge constants when grok.com
    /// rotates its build. [`Self::new`] uses [`ChallengeConfig::default`].
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Transport`] if the underlying HTTP client
    /// fails to build, or [`crate::error::Error::StreamDecode`] if `base_url`
    /// is not a valid URL.
    pub fn with_challenge(
        cookie: GrokCookie,
        network: NetworkConfig,
        challenge: ChallengeConfig,
        runtime: RuntimeState,
    ) -> Result<Self> {
        let base_url = Url::parse(&network.base_url)
            .map_err(|error| Error::StreamDecode(format!("invalid base_url: {error}")))?;
        let cookie_jar = Arc::new(Jar::default());
        seed_cookie_jar(&cookie_jar, &cookie, &base_url);

        let http = reqwest::Client::builder()
            .timeout(network.timeout)
            .gzip(true)
            .brotli(true)
            .zstd(true)
            .deflate(true)
            .cookie_provider(cookie_jar)
            .build()?;
        Ok(Self {
            http,
            network: Arc::new(network),
            challenge: Arc::new(challenge),
            session_ready: Arc::new(OnceCell::new()),
            runtime,
        })
    }

    /// Borrow the runtime state for live defaults lookup.
    #[must_use]
    pub fn runtime(&self) -> &RuntimeState {
        &self.runtime
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.network.base_url
    }

    pub(crate) fn user_agent(&self) -> &str {
        &self.network.user_agent
    }

    pub(crate) fn challenge(&self) -> &ChallengeConfig {
        &self.challenge
    }

    pub(crate) fn stream_idle_timeout(&self) -> Duration {
        self.network.stream_idle_timeout
    }

    /// `GET /rest/subscriptions`.
    pub async fn subscriptions(&self) -> Result<SubscriptionsResponse> {
        self.get_json("/rest/subscriptions").await
    }

    /// `POST /rest/rate-limits` for a given mode.
    pub async fn rate_limits(&self, mode: &Mode) -> Result<RateLimits> {
        self.post_json("/rest/rate-limits", &json!({ "modelName": mode.as_wire() }))
            .await
    }

    /// `POST /rest/skills` — catalog of built-in skills.
    pub async fn skills(&self, locale: &str) -> Result<SkillsResponse> {
        self.post_json("/rest/skills", &json!({ "locale": locale }))
            .await
    }

    /// `GET /rest/assets/<id>`.
    pub async fn get_asset(
        &self,
        file_metadata_id: &crate::models::FileMetadataId,
    ) -> Result<AssetMetadata> {
        let path = format!("/rest/assets/{}", file_metadata_id.as_str());
        self.get_json(&path).await
    }

    pub(crate) async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        self.ensure_browser_session().await?;
        let url = format!("{}{}", self.base_url(), path);
        let (headers, request_id) =
            headers::build_default_headers(self, ACCEPT_JSON, headers::FetchMethod::Get, path)?;
        tracing::debug!(endpoint = %path, %request_id, "GET");
        let response = self.http().get(url).headers(headers).send().await?;
        let body = ensure_success(response, path).await?;
        serde_json::from_slice::<T>(&body).map_err(Error::Serde)
    }

    pub(crate) async fn post_json<B, T>(&self, path: &str, body: &B) -> Result<T>
    where
        B: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        self.ensure_browser_session().await?;
        let url = format!("{}{}", self.base_url(), path);
        let (headers, request_id) =
            headers::build_default_headers(self, ACCEPT_JSON, headers::FetchMethod::Post, path)?;
        tracing::debug!(endpoint = %path, %request_id, "POST");
        let response = self
            .http()
            .post(url)
            .headers(headers)
            .json(body)
            .send()
            .await?;
        let raw = ensure_success(response, path).await?;
        serde_json::from_slice::<T>(&raw).map_err(Error::Serde)
    }

    pub(crate) async fn post_stream<B>(&self, path: &str, body: &B) -> Result<stream::StreamHandle>
    where
        B: Serialize + ?Sized,
    {
        self.ensure_browser_session().await?;
        let url = format!("{}{}", self.base_url(), path);
        let (headers, request_id) =
            headers::build_default_headers(self, ACCEPT_STREAM, headers::FetchMethod::Post, path)?;
        tracing::debug!(endpoint = %path, %request_id, "POST stream");
        let response = self
            .http()
            .post(url)
            .headers(headers)
            .json(body)
            .send()
            .await?;
        let response = ensure_stream_success(response, path).await?;
        Ok(stream::StreamHandle::from_response(
            response,
            self.stream_idle_timeout(),
        ))
    }

    async fn ensure_browser_session(&self) -> Result<()> {
        self.session_ready
            .get_or_try_init(|| async { self.warm_browser_session().await })
            .await
            .map(|_| ())
    }

    async fn warm_browser_session(&self) -> Result<()> {
        let url = format!("{}/", self.base_url().trim_end_matches('/'));
        let headers = headers::build_warmup_headers(self)?;
        let response = self.http().get(url).headers(headers).send().await?;
        let status = response.status();
        let body = response.bytes().await.map_err(Error::Transport)?;
        if status.is_success() {
            return Ok(());
        }
        Err(classify_error(status, &body, "/"))
    }
}

const ACCEPT_JSON: &str = "*/*";
const ACCEPT_STREAM: &str = "*/*";

fn seed_cookie_jar(jar: &Jar, cookie: &GrokCookie, base_url: &Url) {
    for pair in cookie
        .expose()
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
    {
        jar.add_cookie_str(pair, base_url);
    }
}

async fn ensure_success(response: Response, path: &str) -> Result<Vec<u8>> {
    let status = response.status();
    let body = response.bytes().await.map_err(Error::Transport)?;
    if status.is_success() {
        return Ok(body.to_vec());
    }
    Err(classify_error(status, &body, path))
}

async fn ensure_stream_success(response: Response, path: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.bytes().await.map_err(Error::Transport)?;
    Err(classify_error(status, &body, path))
}

fn classify_error(status: StatusCode, body: &[u8], path: &str) -> Error {
    let parsed = parse_api_error(body);
    let preview = preview_body(body);

    match (status, parsed) {
        (StatusCode::UNAUTHORIZED, _) => Error::AuthExpired,
        (StatusCode::FORBIDDEN, Some(api_error)) => {
            if api_error.looks_like_auth_failure() {
                Error::AuthExpired
            } else if api_error.looks_like_anti_bot() {
                Error::AntiBot(api_error)
            } else {
                Error::Forbidden(api_error)
            }
        }
        (StatusCode::FORBIDDEN, None) => Error::UpstreamStatus {
            status: status.as_u16(),
            body: preview,
        },
        (code, Some(api_error)) if code.is_client_error() => Error::Api(api_error),
        (code, _) => {
            tracing::warn!(%code, %path, "grok upstream non-json error");
            Error::UpstreamStatus {
                status: code.as_u16(),
                body: preview,
            }
        }
    }
}

fn parse_api_error(body: &[u8]) -> Option<GrokApiError> {
    serde_json::from_slice::<GrokApiError>(body)
        .ok()
        .or_else(|| {
            serde_json::from_slice::<GrokApiErrorEnvelope>(body)
                .ok()
                .map(|envelope| envelope.error)
        })
}

fn preview_body(bytes: &[u8]) -> String {
    const MAX: usize = 2048;
    let slice = if bytes.len() > MAX {
        &bytes[..MAX]
    } else {
        bytes
    };
    String::from_utf8_lossy(slice).into_owned()
}

#[derive(Debug, Deserialize)]
struct GrokApiErrorEnvelope {
    error: GrokApiError,
}
