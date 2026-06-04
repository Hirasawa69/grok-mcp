//! Build the fixed set of headers grok.com expects on every authenticated call.

use rand::{Rng, RngExt};
use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue, ORIGIN, REFERER,
    USER_AGENT,
};
use uuid::Uuid;

use crate::{
    client::GrokClient,
    config::DEFAULT_USER_AGENT,
    error::{Error, Result},
    models::common::RequestId,
};

const SEC_FETCH_DEST: &str = "empty";
const SEC_FETCH_MODE: &str = "cors";
const SEC_FETCH_SITE: &str = "same-origin";
const SEC_FETCH_DEST_DOCUMENT: &str = "document";
const SEC_FETCH_MODE_NAVIGATE: &str = "navigate";
const SEC_FETCH_SITE_NONE: &str = "none";
const SEC_FETCH_USER: &str = "?1";
const SEC_CH_UA: &str =
    "\"Google Chrome\";v=\"147\", \"Not.A/Brand\";v=\"8\", \"Chromium\";v=\"147\"";
const SEC_CH_UA_MOBILE: &str = "?0";
const SEC_CH_UA_PLATFORM: &str = "\"Linux\"";
const PRIORITY: &str = "u=1, i";
const SENTRY_ENVIRONMENT: &str = "production";
const SENTRY_ORG_ID: &str = "4508179396558848";
const SENTRY_PUBLIC_KEY: &str = "b311e0f2690c81f25e2c4cf6d4f7ce1c";
const SENTRY_RELEASE: &str = "46b985327422759b2b3dadfcf2d9119f430ba00d";
const WARMUP_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8";

#[derive(Clone, Copy)]
pub(crate) enum FetchMethod {
    Get,
    Post,
}

impl FetchMethod {
    /// Upper-case HTTP verb, as used in the `x-statsig-id` signature input.
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// Headers expected by grok.com on every authenticated request.
///
/// * `origin` / `referer` — set to the configured base URL to match web traffic.
/// * `user-agent` / `sec-ch-ua*` / `sec-fetch-*` / `priority` — Chrome-like
///   values matching the observed grok.com web client.
/// * `x-xai-request-id` — fresh UUID per call (grok.com treats this as a
///   client-chosen idempotency-ish marker).
/// * `x-statsig-id` — per-request anti-bot token signed over `(path, method)`
///   via [`crate::client::statsig::ChallengeConfig`]. grok.com rejects requests
///   carrying a random or absent token at the anti-bot layer (`code: 7`), so the
///   `path` must be the exact request path (no query string) the signature is
///   bound to.
/// * `accept` — defaults to `*/*`, matching the grok.com web client.
pub(crate) fn build_default_headers(
    client: &GrokClient,
    accept: &'static str,
    method: FetchMethod,
    path: &str,
) -> Result<(HeaderMap, RequestId)> {
    let mut headers = HeaderMap::new();

    if matches!(method, FetchMethod::Post) {
        let origin_value =
            HeaderValue::from_str(client.base_url()).map_err(|_| invalid_header("origin"))?;
        headers.insert(ORIGIN, origin_value);
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    headers.insert(
        REFERER,
        HeaderValue::from_str(&format!("{}/", client.base_url().trim_end_matches('/')))
            .map_err(|_| invalid_header("referer"))?,
    );

    headers.insert(ACCEPT, HeaderValue::from_static(accept));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US"));
    insert_browser_fetch_headers(client, &mut headers)?;

    let request_id = RequestId::new(Uuid::new_v4().to_string());
    let request_id_header = HeaderName::from_static("x-xai-request-id");
    headers.insert(
        request_id_header,
        HeaderValue::from_str(request_id.as_str()).map_err(|_| invalid_header("request-id"))?,
    );
    let statsig_id = client
        .challenge()
        .generate(signature_path(path), method.as_str());
    headers.insert(
        HeaderName::from_static("x-statsig-id"),
        HeaderValue::from_str(&statsig_id).map_err(|_| invalid_header("x-statsig-id"))?,
    );
    insert_trace_headers(&mut headers)?;

    Ok((headers, request_id))
}

/// The grok.com web client signs `new URL(url).pathname` — the path only, with
/// any query string stripped. Mirror that so the signature matches.
fn signature_path(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

pub(crate) fn build_warmup_headers(client: &GrokClient) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static(WARMUP_ACCEPT));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US"));
    insert_chrome_headers(client, &mut headers)?;
    headers.insert(
        HeaderName::from_static("sec-fetch-site"),
        HeaderValue::from_static(SEC_FETCH_SITE_NONE),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-mode"),
        HeaderValue::from_static(SEC_FETCH_MODE_NAVIGATE),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-dest"),
        HeaderValue::from_static(SEC_FETCH_DEST_DOCUMENT),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-user"),
        HeaderValue::from_static(SEC_FETCH_USER),
    );
    Ok(headers)
}

fn insert_browser_fetch_headers(client: &GrokClient, headers: &mut HeaderMap) -> Result<()> {
    insert_chrome_headers(client, headers)?;
    headers.insert(
        HeaderName::from_static("sec-fetch-site"),
        HeaderValue::from_static(SEC_FETCH_SITE),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-mode"),
        HeaderValue::from_static(SEC_FETCH_MODE),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-dest"),
        HeaderValue::from_static(SEC_FETCH_DEST),
    );
    headers.insert(
        HeaderName::from_static("priority"),
        HeaderValue::from_static(PRIORITY),
    );
    Ok(())
}

fn insert_chrome_headers(client: &GrokClient, headers: &mut HeaderMap) -> Result<()> {
    headers.insert(
        USER_AGENT,
        HeaderValue::from_str(resolve_user_agent(client))
            .map_err(|_| invalid_header("user-agent"))?,
    );
    headers.insert(
        HeaderName::from_static("sec-ch-ua"),
        HeaderValue::from_static(SEC_CH_UA),
    );
    headers.insert(
        HeaderName::from_static("sec-ch-ua-mobile"),
        HeaderValue::from_static(SEC_CH_UA_MOBILE),
    );
    headers.insert(
        HeaderName::from_static("sec-ch-ua-platform"),
        HeaderValue::from_static(SEC_CH_UA_PLATFORM),
    );
    Ok(())
}

fn resolve_user_agent(client: &GrokClient) -> &str {
    let user_agent = client.user_agent();
    if user_agent.eq_ignore_ascii_case("<auto>") {
        return DEFAULT_USER_AGENT;
    }
    user_agent
}

fn insert_trace_headers(headers: &mut HeaderMap) -> Result<()> {
    let traceparent_trace_id = random_hex::<16>();
    let traceparent_span_id = random_hex::<8>();
    let sentry_trace_id = random_hex::<16>();
    let sentry_span_id = random_hex::<8>();
    let sample_rand = rand::rng().random_range(0.0..1.0);

    let traceparent = format!("00-{traceparent_trace_id}-{traceparent_span_id}-00");
    let sentry_trace = format!("{sentry_trace_id}-{sentry_span_id}-0");
    let baggage = format!(
        "sentry-environment={SENTRY_ENVIRONMENT},sentry-release={SENTRY_RELEASE},\
         sentry-public_key={SENTRY_PUBLIC_KEY},sentry-trace_id={sentry_trace_id},\
         sentry-org_id={SENTRY_ORG_ID},sentry-sampled=false,\
         sentry-sample_rand={sample_rand},sentry-sample_rate=0"
    );

    headers.insert(
        HeaderName::from_static("traceparent"),
        HeaderValue::from_str(&traceparent).map_err(|_| invalid_header("traceparent"))?,
    );
    headers.insert(
        HeaderName::from_static("sentry-trace"),
        HeaderValue::from_str(&sentry_trace).map_err(|_| invalid_header("sentry-trace"))?,
    );
    headers.insert(
        HeaderName::from_static("baggage"),
        HeaderValue::from_str(&baggage).map_err(|_| invalid_header("baggage"))?,
    );
    Ok(())
}

fn random_hex<const N: usize>() -> String {
    let mut bytes = [0_u8; N];
    rand::rng().fill_bytes(&mut bytes);
    bytes
        .iter()
        .flat_map(|byte| [HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]])
        .map(char::from)
        .collect()
}

fn invalid_header(name: &'static str) -> Error {
    Error::StreamDecode(format!("failed to build header: {name}"))
}

const HEX: &[u8; 16] = b"0123456789abcdef";

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        config::{ChatDefaults, NetworkConfig, RuntimeState},
        cookie::GrokCookie,
    };

    fn test_client(network: NetworkConfig) -> GrokClient {
        let cookie = GrokCookie::parse("sso=aaa; sso-rw=bbb").expect("test cookie should parse");
        let runtime = RuntimeState::new(ChatDefaults::default());
        GrokClient::new(cookie, network, runtime).expect("test client should build")
    }

    fn test_network(user_agent: &str) -> NetworkConfig {
        NetworkConfig {
            base_url: "https://grok.com".to_owned(),
            user_agent: user_agent.to_owned(),
            timeout: Duration::from_secs(120),
            stream_idle_timeout: Duration::from_secs(60),
        }
    }

    fn assert_statsig_shape(encoded: &str) {
        use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
        let decoded = STANDARD_NO_PAD
            .decode(encoded)
            .expect("x-statsig-id should be valid base64");
        assert_eq!(decoded.len(), 70);
        assert_eq!(encoded.len(), 94);
        assert!(!encoded.ends_with('='));
    }

    #[test]
    fn build_default_headers_adds_browser_and_cloudflare_headers() {
        let client = test_client(test_network("<AuTo>"));

        let (headers, request_id) = build_default_headers(
            &client,
            "*/*",
            FetchMethod::Post,
            "/rest/app-chat/conversations/new",
        )
        .expect("headers");

        assert_eq!(headers.get(ORIGIN).expect("origin"), "https://grok.com");
        assert_eq!(headers.get(REFERER).expect("referer"), "https://grok.com/");
        assert_eq!(headers.get(ACCEPT).expect("accept"), "*/*");
        assert_eq!(
            headers.get(ACCEPT_LANGUAGE).expect("accept-language"),
            "en-US"
        );
        assert_eq!(
            headers.get(USER_AGENT).expect("user-agent"),
            DEFAULT_USER_AGENT
        );
        assert_eq!(headers.get("sec-ch-ua").expect("sec-ch-ua"), SEC_CH_UA);
        assert_eq!(
            headers.get("sec-ch-ua-mobile").expect("sec-ch-ua-mobile"),
            SEC_CH_UA_MOBILE
        );
        assert_eq!(
            headers
                .get("sec-ch-ua-platform")
                .expect("sec-ch-ua-platform"),
            SEC_CH_UA_PLATFORM
        );
        assert_eq!(
            headers.get("sec-fetch-site").expect("sec-fetch-site"),
            SEC_FETCH_SITE
        );
        assert_eq!(
            headers.get("sec-fetch-mode").expect("sec-fetch-mode"),
            SEC_FETCH_MODE
        );
        assert_eq!(
            headers.get("sec-fetch-dest").expect("sec-fetch-dest"),
            SEC_FETCH_DEST
        );
        assert_eq!(
            headers.get(CONTENT_TYPE).expect("content-type"),
            "application/json"
        );
        assert_eq!(headers.get("priority").expect("priority"), PRIORITY);
        assert!(headers.get("traceparent").is_some());
        assert!(headers.get("sentry-trace").is_some());
        assert!(headers.get("baggage").is_some());
        assert_eq!(
            headers.get("x-xai-request-id").expect("x-xai-request-id"),
            request_id.as_str()
        );

        let statsig = headers
            .get("x-statsig-id")
            .expect("x-statsig-id")
            .to_str()
            .expect("x-statsig-id should be ascii");
        assert_statsig_shape(statsig);
    }

    #[test]
    fn build_default_headers_uses_custom_user_agent_and_fresh_request_ids() {
        let client = test_client(test_network("Custom Browser/1.0"));

        let (headers_a, request_id_a) = build_default_headers(
            &client,
            "application/json, text/plain, */*",
            FetchMethod::Post,
            "/rest/app-chat/conversations/new",
        )
        .expect("first header set");
        let (headers_b, request_id_b) = build_default_headers(
            &client,
            "application/json, text/plain, */*",
            FetchMethod::Post,
            "/rest/app-chat/conversations/new",
        )
        .expect("second header set");

        assert_eq!(
            headers_a.get(USER_AGENT).expect("user-agent"),
            "Custom Browser/1.0"
        );
        assert_eq!(
            headers_a.get(ACCEPT).expect("accept"),
            "application/json, text/plain, */*"
        );
        assert_eq!(
            headers_a.get("x-xai-request-id").expect("x-xai-request-id"),
            request_id_a.as_str()
        );
        assert_eq!(
            headers_b.get("x-xai-request-id").expect("x-xai-request-id"),
            request_id_b.as_str()
        );
        assert_ne!(
            request_id_a, request_id_b,
            "request ids should be fresh per request"
        );

        let statsig_a = headers_a
            .get("x-statsig-id")
            .expect("x-statsig-id")
            .to_str()
            .expect("x-statsig-id should be ascii");
        let statsig_b = headers_b
            .get("x-statsig-id")
            .expect("x-statsig-id")
            .to_str()
            .expect("x-statsig-id should be ascii");
        assert_statsig_shape(statsig_a);
        assert_statsig_shape(statsig_b);
        assert_ne!(
            statsig_a, statsig_b,
            "x-statsig-id should be fresh per request"
        );
    }

    #[test]
    fn get_headers_omit_post_only_headers() {
        let client = test_client(test_network("<auto>"));

        let (headers, _) = build_default_headers(
            &client,
            "*/*",
            FetchMethod::Get,
            "/rest/app-chat/conversations",
        )
        .expect("headers");

        assert!(headers.get(ORIGIN).is_none());
        assert!(headers.get(CONTENT_TYPE).is_none());
        assert_eq!(headers.get(ACCEPT).expect("accept"), "*/*");
    }

    #[test]
    fn random_hex_returns_lowercase_hex() {
        let value = random_hex::<16>();

        assert_eq!(value.len(), 32);
        assert!(value.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(value, value.to_ascii_lowercase());
    }
}
