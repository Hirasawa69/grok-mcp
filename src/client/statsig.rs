//! `x-statsig-id` anti-bot token generation for grok.com.
//!
//! grok.com signs every authenticated `/rest/*` request with an `x-statsig-id`
//! header. The grok.com web client computes it from an obfuscated, lazy-loaded
//! challenge bundle; requests carrying a random or absent token are rejected at
//! the anti-bot layer with `{code: 7, message: "Request rejected by anti-bot
//! rules."}` even when the session cookies are valid.
//!
//! The token is a 70-byte little-endian record, XOR-masked with a single random
//! byte and base64-encoded (standard alphabet, no padding):
//!
//! ```text
//! raw[70] = header[49]            // build-specific static fingerprint
//!         | counter_le32[4]       // floor(unix_secs) - EPOCH, little-endian u32
//!         | sha256(sig_input)[..16]
//!         | trailer[1]            // build-specific constant byte
//! sig_input = format!("{METHOD}!{path}!{counter}{suffix}")
//! token = base64_no_pad(raw[i] ^ key) for a fresh random key byte
//! ```
//!
//! The three build-specific constants (`header`, `suffix`, `trailer`) are baked
//! into the challenge bundle and rotate when grok.com ships a new build. They
//! live in [`ChallengeConfig`] so they can be refreshed via config without a
//! rebuild. See `README.md` for the browser-console extraction snippet.

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD as BASE64_STANDARD_NO_PAD};
use rand::RngExt;
use sha2::{Digest, Sha256};

use crate::error::{ConfigError, Error, Result};

/// Seconds between the Unix epoch and the challenge epoch (2023-05-01 00:00 UTC).
const EPOCH: u64 = 1_682_924_400;

const HEADER_LEN: usize = 49;
const COUNTER_LEN: usize = 4;
const HASH_LEN: usize = 16;
const TRAILER_LEN: usize = 1;
const TOKEN_LEN: usize = HEADER_LEN + COUNTER_LEN + HASH_LEN + TRAILER_LEN;
const HASH_START: usize = HEADER_LEN + COUNTER_LEN;

/// Build-specific constants for the grok.com `x-statsig-id` challenge.
///
/// Defaults track the grok.com build observed at implementation time. Override
/// via the `[challenge]` config table when grok.com rotates them.
#[derive(Clone)]
pub struct ChallengeConfig {
    header: [u8; HEADER_LEN],
    suffix: String,
    trailer: u8,
}

impl ChallengeConfig {
    /// Build a config from a hex-encoded 49-byte header, a suffix string, and a
    /// trailer byte.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] if `header_hex` is not valid hex or does not
    /// decode to exactly [`HEADER_LEN`] bytes.
    pub fn new(header_hex: &str, suffix: impl Into<String>, trailer: u8) -> Result<Self> {
        let decoded = decode_hex(header_hex)?;
        let header = <[u8; HEADER_LEN]>::try_from(decoded.as_slice()).map_err(|_| {
            Error::Config(ConfigError::InvalidEnv {
                name: "challenge.header_hex",
                value: format!("expected {HEADER_LEN} bytes, got {}", decoded.len()),
            })
        })?;
        Ok(Self {
            header,
            suffix: suffix.into(),
            trailer,
        })
    }

    /// Generate a fresh `x-statsig-id` token for the given request path + method.
    ///
    /// `path` must be the URL path only (no scheme/host/query), e.g.
    /// `/rest/app-chat/conversations/new`. `method` is the upper-case HTTP verb.
    #[must_use]
    pub fn generate(&self, path: &str, method: &str) -> String {
        let counter = now_counter();
        let signature = format!("{method}!{path}!{counter}{}", self.suffix);
        let hash = Sha256::digest(signature.as_bytes());

        let mut raw = [0_u8; TOKEN_LEN];
        raw[..HEADER_LEN].copy_from_slice(&self.header);
        raw[HEADER_LEN..HASH_START].copy_from_slice(&(counter as u32).to_le_bytes());
        raw[HASH_START..HASH_START + HASH_LEN].copy_from_slice(&hash[..HASH_LEN]);
        raw[TOKEN_LEN - 1] = self.trailer;

        let key = rand::rng().random::<u8>();
        for byte in &mut raw {
            *byte ^= key;
        }

        BASE64_STANDARD_NO_PAD.encode(raw)
    }
}

impl Default for ChallengeConfig {
    fn default() -> Self {
        Self {
            header: DEFAULT_HEADER,
            suffix: DEFAULT_SUFFIX.to_owned(),
            trailer: DEFAULT_TRAILER,
        }
    }
}

impl std::fmt::Debug for ChallengeConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChallengeConfig")
            .field("header_len", &self.header.len())
            .field("suffix_len", &self.suffix.len())
            .field("trailer", &self.trailer)
            .finish()
    }
}

fn now_counter() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(EPOCH)
        .saturating_sub(EPOCH)
}

fn decode_hex(hex: &str) -> Result<Vec<u8>> {
    let invalid = |value: &str| {
        Error::Config(ConfigError::InvalidEnv {
            name: "challenge.header_hex",
            value: value.to_owned(),
        })
    };
    if !hex.len().is_multiple_of(2) {
        return Err(invalid("odd-length hex string"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).map_err(|_| invalid(hex)))
        .collect::<Result<Vec<u8>>>()
}

/// Default 49-byte challenge header for the grok.com build observed at
/// implementation time.
const DEFAULT_HEADER: [u8; HEADER_LEN] = [
    0, 122, 70, 242, 98, 14, 77, 131, 83, 59, 177, 38, 245, 253, 115, 245, 175, 81, 112, 97, 208,
    249, 85, 40, 164, 211, 58, 50, 153, 80, 71, 147, 212, 236, 53, 119, 26, 174, 57, 154, 194, 36,
    75, 132, 237, 238, 165, 2, 144,
];

/// Default challenge suffix for the grok.com build observed at implementation time.
const DEFAULT_SUFFIX: &str = "obfiowerehiringbd855e10170a3d70a3d70a0170a3d70a3d70a100";

/// Default challenge trailer byte for the grok.com build observed at implementation time.
const DEFAULT_TRAILER: u8 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_builds() {
        let config = ChallengeConfig::default();
        assert_eq!(config.header.len(), HEADER_LEN);
        assert_eq!(config.trailer, DEFAULT_TRAILER);
    }

    #[test]
    fn token_decodes_to_expected_length() {
        let config = ChallengeConfig::default();
        let token = config.generate("/rest/app-chat/conversations/new", "POST");
        let decoded = BASE64_STANDARD_NO_PAD
            .decode(token.as_bytes())
            .expect("token is valid base64");
        assert_eq!(decoded.len(), TOKEN_LEN);
    }

    #[test]
    fn token_recovers_header_hash_and_trailer_after_xor() {
        // The whole record is masked with one random byte. The first header byte
        // is known (0x00), so XOR with the emitted first byte recovers the key,
        // letting us verify the embedded layout matches the grok.com algorithm.
        let config = ChallengeConfig::default();
        let path = "/rest/app-chat/conversations";
        let method = "GET";
        let token = config.generate(path, method);
        let decoded = BASE64_STANDARD_NO_PAD
            .decode(token.as_bytes())
            .expect("valid base64");

        let key = decoded[0] ^ DEFAULT_HEADER[0];
        let unmasked = decoded.iter().map(|byte| byte ^ key).collect::<Vec<u8>>();

        assert_eq!(&unmasked[..HEADER_LEN], &DEFAULT_HEADER);
        assert_eq!(unmasked[TOKEN_LEN - 1], DEFAULT_TRAILER);

        let counter = u32::from_le_bytes(
            <[u8; 4]>::try_from(&unmasked[HEADER_LEN..HASH_START]).expect("counter slice"),
        );
        let expected =
            Sha256::digest(format!("{method}!{path}!{counter}{DEFAULT_SUFFIX}").as_bytes());
        assert_eq!(
            &unmasked[HASH_START..HASH_START + HASH_LEN],
            &expected[..HASH_LEN]
        );
    }

    #[test]
    fn tokens_vary_across_calls() {
        let config = ChallengeConfig::default();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..16 {
            seen.insert(config.generate("/rest/x", "POST"));
        }
        assert!(seen.len() > 1, "xor masking should vary the token");
    }

    #[test]
    fn new_rejects_wrong_header_length() {
        let error = ChallengeConfig::new("00", "suffix", 3).expect_err("too short");
        assert!(matches!(
            error,
            Error::Config(ConfigError::InvalidEnv { .. })
        ));
    }

    #[test]
    fn new_rejects_odd_hex() {
        let error = ChallengeConfig::new("0", "suffix", 3).expect_err("odd length");
        assert!(matches!(
            error,
            Error::Config(ConfigError::InvalidEnv { .. })
        ));
    }

    #[test]
    fn new_round_trips_valid_header() {
        let hex = "00".repeat(HEADER_LEN);
        let config = ChallengeConfig::new(&hex, "suffix", 7).expect("valid header");
        assert_eq!(config.trailer, 7);
        assert_eq!(config.header, [0_u8; HEADER_LEN]);
    }
}
