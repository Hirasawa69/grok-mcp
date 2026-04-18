//! Cookie-header handling for grok.com authentication.
//!
//! The raw cookie string is wrapped in [`secrecy::SecretString`] so it never
//! leaks into `Debug` output or logs. Construction goes through [`GrokCookie::parse`]
//! which verifies that the required session carriers (`sso`, `sso-rw`) are present
//! and warns-but-accepts when the Cloudflare / user-id companions are missing.

use std::fmt;

use secrecy::{ExposeSecret, SecretString};

use crate::error::CookieError;

/// Cookie names strictly required to reach authenticated grok.com endpoints.
///
/// Two independent reverse-engineering audits agree these are the session-token
/// carriers. Missing either one means no valid session at all.
pub const REQUIRED_COOKIES: &[&str] = &["sso", "sso-rw"];

/// Cookie names that accompany `sso` / `sso-rw` in observed grok.com traffic
/// but may be WAF-optional depending on the user's network. We warn but do not
/// fail when they are missing.
pub const WARN_COOKIES: &[&str] = &["x-userid", "cf_clearance", "__cf_bm"];

/// Validated cookie header ready to be attached to outgoing requests.
///
/// The raw string is held in a [`SecretString`]; [`Self::expose`] is the only way
/// to read it back. `Debug` prints `GrokCookie(<redacted>)` so it cannot leak via
/// log macros or panic traces.
#[derive(Clone)]
pub struct GrokCookie {
    header: SecretString,
}

impl GrokCookie {
    /// Parse and validate a browser-copied cookie header.
    ///
    /// # Errors
    ///
    /// * [`CookieError::Empty`] — input contained no non-whitespace bytes.
    /// * [`CookieError::Malformed`] — at least one pair was not `name=value`.
    /// * [`CookieError::MissingRequired`] — `sso` or `sso-rw` was absent.
    pub fn parse(raw: &str) -> Result<Self, CookieError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(CookieError::Empty);
        }

        let pairs = parse_pairs(trimmed)?;

        for required in REQUIRED_COOKIES {
            let present = pairs
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(required));
            if !present {
                return Err(CookieError::MissingRequired(required));
            }
        }

        for warn_only in WARN_COOKIES {
            let present = pairs
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(warn_only));
            if !present {
                tracing::warn!(
                    cookie = warn_only,
                    "cookie missing from header; request may still succeed but this cookie is usually present alongside the session"
                );
            }
        }

        let canonical = pairs
            .into_iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<String>>()
            .join("; ");

        Ok(Self {
            header: SecretString::from(canonical),
        })
    }

    /// Expose the cookie header as a `&str` for attaching to an outgoing request.
    ///
    /// The returned slice is secret — it must not be logged or stored
    /// anywhere that outlives the request it is attached to.
    #[must_use]
    pub fn expose(&self) -> &str {
        self.header.expose_secret()
    }
}

impl fmt::Debug for GrokCookie {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("GrokCookie")
            .field(&"<redacted>")
            .finish()
    }
}

fn parse_pairs(raw: &str) -> Result<Vec<(String, String)>, CookieError> {
    let mut out = Vec::new();
    for chunk in raw.split(';') {
        let pair = chunk.trim();
        if pair.is_empty() {
            continue;
        }
        let (name, value) = pair.split_once('=').ok_or(CookieError::Malformed)?;
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            return Err(CookieError::Malformed);
        }
        out.push((name.to_owned(), value.to_owned()));
    }
    if out.is_empty() {
        return Err(CookieError::Empty);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_happy_path() {
        let cookie =
            GrokCookie::parse("sso=aaa; sso-rw=bbb; x-userid=uid; cf_clearance=xx; __cf_bm=yy")
                .expect("valid cookie");
        assert!(cookie.expose().contains("sso=aaa"));
        assert!(cookie.expose().contains("sso-rw=bbb"));
    }

    #[test]
    fn parse_accepts_missing_warn_cookies() {
        let cookie = GrokCookie::parse("sso=aaa; sso-rw=bbb").expect("required cookies present");
        assert!(cookie.expose().contains("sso-rw=bbb"));
    }

    #[test]
    fn parse_rejects_missing_required() {
        let error = GrokCookie::parse("sso=only").expect_err("missing sso-rw");
        assert!(matches!(error, CookieError::MissingRequired("sso-rw")));
    }

    #[test]
    fn parse_rejects_empty() {
        let error = GrokCookie::parse("   ").expect_err("empty");
        assert!(matches!(error, CookieError::Empty));
    }

    #[test]
    fn parse_rejects_malformed() {
        let error = GrokCookie::parse("sso").expect_err("no =");
        assert!(matches!(error, CookieError::Malformed));
    }

    #[test]
    fn debug_never_leaks_secret() {
        let cookie = GrokCookie::parse("sso=secret; sso-rw=also").expect("ok");
        let debug = format!("{cookie:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("<redacted>"));
    }
}
