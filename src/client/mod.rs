//! HTTP client for the grok.com REST + NDJSON surface.
//!
//! The [`GrokClient`] owns a [`reqwest::Client`] plus the base URL, User-Agent,
//! cookie header, and a handle to [`crate::config::RuntimeState`] for
//! effective-defaults lookups. Simple direct methods live here; resource
//! builders live under [`conversations`] and [`uploads`]. The streaming parser
//! lives on [`stream`].

pub mod conversations;
pub mod headers;
pub mod stream;
pub mod uploads;

use std::{sync::Arc, time::Duration};

use reqwest::{Response, StatusCode};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

use crate::{
    config::{NetworkConfig, RuntimeState},
    cookie::GrokCookie,
    error::{Error, Result},
    models::{
        AssetMetadata, GrokApiError, Mode, RateLimits, SkillsResponse, SubscriptionsResponse,
    },
};

/// Thin wrapper around [`reqwest::Client`] with grok.com-specific wiring.
///
/// Clone is cheap — the inner `reqwest::Client` and `GrokCookie` share state via
/// `Arc`, and [`RuntimeState`] already wraps its own `Arc`.
#[derive(Clone)]
pub struct GrokClient {
    http: reqwest::Client,
    network: Arc<NetworkConfig>,
    cookie: GrokCookie,
    runtime: RuntimeState,
}

impl GrokClient {
    /// Build a new client with a pre-validated cookie and network config.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::Error::Transport`] if the underlying
    /// `reqwest::Client` fails to build (TLS init, etc.).
    pub fn new(cookie: GrokCookie, network: NetworkConfig, runtime: RuntimeState) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(network.timeout)
            .gzip(true)
            .cookie_store(true)
            .build()?;
        Ok(Self {
            http,
            network: Arc::new(network),
            cookie,
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

    pub(crate) fn cookie(&self) -> &GrokCookie {
        &self.cookie
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.network.base_url
    }

    pub(crate) fn user_agent(&self) -> &str {
        &self.network.user_agent
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
        let url = format!("{}{}", self.base_url(), path);
        let (headers, request_id) = headers::build_default_headers(self, ACCEPT_JSON)?;
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
        let url = format!("{}{}", self.base_url(), path);
        let (headers, request_id) = headers::build_default_headers(self, ACCEPT_JSON)?;
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
        let url = format!("{}{}", self.base_url(), path);
        let (headers, request_id) = headers::build_default_headers(self, ACCEPT_STREAM)?;
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
}

const ACCEPT_JSON: &str = "application/json, text/plain, */*";
const ACCEPT_STREAM: &str = "*/*";

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
    let parsed = serde_json::from_slice::<GrokApiError>(body).ok();
    let preview = preview_body(body);

    match (status, parsed) {
        (StatusCode::UNAUTHORIZED, _) => Error::AuthExpired,
        (StatusCode::FORBIDDEN, Some(api_error)) => {
            if api_error.looks_like_auth_failure() {
                Error::AuthExpired
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

fn preview_body(bytes: &[u8]) -> String {
    const MAX: usize = 2048;
    let slice = if bytes.len() > MAX {
        &bytes[..MAX]
    } else {
        bytes
    };
    String::from_utf8_lossy(slice).into_owned()
}
