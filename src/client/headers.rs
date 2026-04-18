//! Build the fixed set of headers grok.com expects on every authenticated call.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rand::{Rng, RngExt};
use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, COOKIE, HeaderMap, HeaderName, HeaderValue, ORIGIN,
    REFERER, USER_AGENT,
};
use uuid::Uuid;

use crate::{
    client::GrokClient,
    config::DEFAULT_USER_AGENT,
    error::{Error, Result},
    models::common::RequestId,
};

const SEC_CH_UA: &str =
    "\"Not;A=Brand\";v=\"99\", \"Google Chrome\";v=\"139\", \"Chromium\";v=\"139\"";
const SEC_CH_UA_MOBILE: &str = "?0";
const SEC_CH_UA_PLATFORM: &str = "\"Windows\"";
const SEC_FETCH_DEST: &str = "empty";
const SEC_FETCH_MODE: &str = "cors";
const SEC_FETCH_SITE: &str = "same-origin";

/// Headers expected by grok.com on every authenticated request.
///
/// * `cookie` — the session carrier, attached verbatim.
/// * `origin` / `referer` — set to the configured base URL to match web traffic.
/// * `user-agent` / `sec-ch-ua*` / `sec-fetch-*` — browser-like values to
///   match the real grok.com web client closely enough to clear Cloudflare.
/// * `x-xai-request-id` — fresh UUID per call (grok.com treats this as a
///   client-chosen idempotency-ish marker).
/// * `x-statsig-id` — fresh per-call bot-management token shaped like the web
///   client's JS-generated base64 TypeError probe.
/// * `accept` — defaults to `application/json, text/plain, */*`, matching the
///   grok.com web client.
pub(crate) fn build_default_headers(
    client: &GrokClient,
    accept: &'static str,
) -> Result<(HeaderMap, RequestId)> {
    let mut headers = HeaderMap::new();

    let cookie_value =
        HeaderValue::from_str(client.cookie().expose()).map_err(|_| invalid_header("cookie"))?;
    headers.insert(COOKIE, cookie_value);

    let origin_value =
        HeaderValue::from_str(client.base_url()).map_err(|_| invalid_header("origin"))?;
    headers.insert(ORIGIN, origin_value.clone());
    headers.insert(REFERER, origin_value);

    headers.insert(ACCEPT, HeaderValue::from_static(accept));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
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

    let request_id = RequestId::new(Uuid::new_v4().to_string());
    let request_id_header = HeaderName::from_static("x-xai-request-id");
    headers.insert(
        request_id_header,
        HeaderValue::from_str(request_id.as_str()).map_err(|_| invalid_header("request-id"))?,
    );
    headers.insert(
        HeaderName::from_static("x-statsig-id"),
        HeaderValue::from_str(&gen_statsig_id()).map_err(|_| invalid_header("x-statsig-id"))?,
    );

    Ok((headers, request_id))
}

fn resolve_user_agent(client: &GrokClient) -> &str {
    let user_agent = client.user_agent();
    if user_agent.eq_ignore_ascii_case("<auto>") {
        return DEFAULT_USER_AGENT;
    }
    user_agent
}

fn gen_statsig_id() -> String {
    let mut rng = rand::rng();
    let message = if rng.random_bool(0.5) {
        format!(
            "e:TypeError: Cannot read properties of null (reading 'children[\'{}\']')",
            pick_random(&mut rng, b"abcdefghijklmnopqrstuvwxyz0123456789", 5),
        )
    } else {
        format!(
            "e:TypeError: Cannot read properties of undefined (reading '{}')",
            pick_random(&mut rng, b"abcdefghijklmnopqrstuvwxyz", 10),
        )
    };
    BASE64_STANDARD.encode(message)
}

fn pick_random<R>(rng: &mut R, pool: &[u8], len: usize) -> String
where
    R: Rng + ?Sized,
{
    (0..len)
        .map(|_| pool[rng.random_range(0..pool.len())] as char)
        .collect::<String>()
}

fn invalid_header(name: &'static str) -> Error {
    Error::StreamDecode(format!("failed to build header: {name}"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::{
        config::{ChatDefaults, NetworkConfig, RuntimeState},
        cookie::GrokCookie,
    };

    fn test_client(user_agent: &str) -> GrokClient {
        let cookie = GrokCookie::parse("sso=aaa; sso-rw=bbb").expect("test cookie should parse");
        let network = NetworkConfig {
            base_url: "https://grok.com".to_owned(),
            user_agent: user_agent.to_owned(),
            timeout: Duration::from_secs(120),
            stream_idle_timeout: Duration::from_secs(60),
        };
        let runtime = RuntimeState::new(ChatDefaults::default());
        GrokClient::new(cookie, network, runtime).expect("test client should build")
    }

    fn assert_statsig_shape(encoded: &str) {
        let decoded = BASE64_STANDARD
            .decode(encoded)
            .expect("x-statsig-id should be valid base64");
        let message = String::from_utf8(decoded).expect("x-statsig-id should decode to utf-8");

        if let Some(inner) = message
            .strip_prefix("e:TypeError: Cannot read properties of null (reading 'children['")
            .and_then(|rest| rest.strip_suffix("']')"))
        {
            assert_eq!(inner.len(), 5);
            assert!(
                inner
                    .chars()
                    .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit())
            );
            return;
        }

        if let Some(inner) = message
            .strip_prefix("e:TypeError: Cannot read properties of undefined (reading '")
            .and_then(|rest| rest.strip_suffix("')"))
        {
            assert_eq!(inner.len(), 10);
            assert!(
                inner
                    .chars()
                    .all(|character| character.is_ascii_lowercase())
            );
            return;
        }

        panic!("unexpected x-statsig-id payload: {message}");
    }

    #[test]
    fn build_default_headers_adds_browser_and_cloudflare_headers() {
        let client = test_client("<AuTo>");

        let (headers, request_id) = build_default_headers(&client, "*/*").expect("headers");

        assert_eq!(headers.get(COOKIE).expect("cookie"), "sso=aaa; sso-rw=bbb");
        assert_eq!(headers.get(ORIGIN).expect("origin"), "https://grok.com");
        assert_eq!(headers.get(REFERER).expect("referer"), "https://grok.com");
        assert_eq!(headers.get(ACCEPT).expect("accept"), "*/*");
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
        let client = test_client("Custom Browser/1.0");

        let (headers_a, request_id_a) =
            build_default_headers(&client, "application/json, text/plain, */*")
                .expect("first header set");
        let (headers_b, request_id_b) =
            build_default_headers(&client, "application/json, text/plain, */*")
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
    }
}
