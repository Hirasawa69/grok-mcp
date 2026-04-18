//! Layered configuration: hard-coded defaults, TOML file, env vars.
//!
//! Precedence, lowest → highest:
//!
//! 1. [`ChatDefaults::default`]
//! 2. `~/.config/grok-mcp/config.toml` (path overridable via `GROK_MCP_CONFIG`)
//! 3. Individual env vars (`GROK_COOKIE`, `GROK_MODE`, …)
//! 4. Runtime mutation via the `grok_set_defaults` MCP tool (not persisted)

pub mod defaults;
pub mod runtime;

use std::{
    env,
    path::{Path, PathBuf},
    time::Duration,
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{
    cookie::GrokCookie,
    error::{ConfigError, Error, Result},
    models::common::Mode,
};

pub use defaults::ChatDefaults;
pub use runtime::RuntimeState;

#[rustfmt::skip]
pub(crate) const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/139.0.0.0 Safari/537.36";

/// Fully resolved configuration, ready to build a client and server from.
#[derive(Debug)]
pub struct Config {
    pub cookie: GrokCookie,
    pub defaults: ChatDefaults,
    pub network: NetworkConfig,
}

/// Network / transport tuning. Defaults mirror the grok.com web client.
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub base_url: String,
    pub user_agent: String,
    pub timeout: Duration,
    pub stream_idle_timeout: Duration,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            base_url: "https://grok.com".to_owned(),
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            timeout: Duration::from_secs(120),
            stream_idle_timeout: Duration::from_secs(60),
        }
    }
}

impl Config {
    /// Load config from the default location + environment.
    ///
    /// The default location is `GROK_MCP_CONFIG` if set, otherwise the
    /// platform config dir (`~/.config/grok-mcp/config.toml` on Linux).
    ///
    /// # Errors
    ///
    /// * [`Error::Config`] if the config file is unreadable or malformed, or if
    ///   a required value (cookie header) ends up missing after layering.
    /// * [`Error::Cookie`] if the resolved cookie header fails validation.
    pub fn load() -> Result<Self> {
        Self::load_with_override(None)
    }

    /// Load config from an explicit file path, falling back to the default
    /// location if `override_path` is `None`.
    ///
    /// # Errors
    ///
    /// Same as [`Self::load`].
    pub fn load_with_override(override_path: Option<&Path>) -> Result<Self> {
        let file_config = load_file_config(override_path)?;
        let env_config = EnvConfig::from_env()?;

        let cookie_raw = env_config
            .cookie
            .or(file_config.cookie)
            .ok_or_else(|| Error::Config(ConfigError::MissingRequired("cookie")))?;
        let cookie = GrokCookie::parse(&cookie_raw)?;

        let mut defaults = file_config.defaults.unwrap_or_default();
        env_config.defaults.apply_to(&mut defaults);

        let mut network = file_config.network.unwrap_or_default();
        if let Some(base_url) = env_config.base_url {
            network.base_url = base_url;
        }
        if let Some(timeout) = env_config.timeout {
            network.timeout = timeout;
        }

        Ok(Self {
            cookie,
            defaults,
            network,
        })
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct FileConfig {
    #[serde(default)]
    cookie: Option<String>,
    #[serde(default)]
    defaults: Option<ChatDefaults>,
    #[serde(default)]
    network: Option<NetworkFileConfig>,
}

#[derive(Debug, Deserialize, Serialize)]
struct NetworkFileConfig {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    stream_idle_timeout_seconds: Option<u64>,
}

impl FileConfig {
    fn network_resolved(self) -> ResolvedFileConfig {
        ResolvedFileConfig {
            cookie: self.cookie,
            defaults: self.defaults,
            network: Some(resolve_network(self.network)),
        }
    }
}

fn resolve_network(network: Option<NetworkFileConfig>) -> NetworkConfig {
    let mut base = NetworkConfig::default();
    let Some(file) = network else {
        return base;
    };
    if let Some(url) = file.base_url {
        base.base_url = url;
    }
    if let Some(agent) = file.user_agent
        && !agent.eq_ignore_ascii_case("<auto>")
    {
        base.user_agent = agent;
    }
    if let Some(seconds) = file.timeout_seconds {
        base.timeout = Duration::from_secs(seconds);
    }
    if let Some(seconds) = file.stream_idle_timeout_seconds {
        base.stream_idle_timeout = Duration::from_secs(seconds);
    }
    base
}

#[derive(Debug, Default)]
struct ResolvedFileConfig {
    cookie: Option<String>,
    defaults: Option<ChatDefaults>,
    network: Option<NetworkConfig>,
}

fn load_file_config(override_path: Option<&Path>) -> Result<ResolvedFileConfig> {
    let path = match override_path {
        Some(explicit) => Some(explicit.to_path_buf()),
        None => resolve_config_path(),
    };
    let Some(path) = path else {
        return Ok(ResolvedFileConfig::default());
    };
    if !path.exists() {
        // A missing default path means "no file config"; a missing explicit
        // override was spelled by the user and must surface as an error.
        if override_path.is_some() {
            return Err(Error::Config(ConfigError::ReadFile {
                path: path.display().to_string(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "config file not found"),
            }));
        }
        return Ok(ResolvedFileConfig::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|source| {
        Error::Config(ConfigError::ReadFile {
            path: path.display().to_string(),
            source,
        })
    })?;
    let parsed = toml::from_str::<FileConfig>(&text).map_err(ConfigError::Toml)?;
    Ok(parsed.network_resolved())
}

/// Resolve the default config path: `GROK_MCP_CONFIG` env → platform config dir.
pub fn default_config_path() -> Option<PathBuf> {
    resolve_config_path()
}

fn resolve_config_path() -> Option<PathBuf> {
    if let Ok(override_path) = env::var("GROK_MCP_CONFIG") {
        return Some(PathBuf::from(override_path));
    }
    let dirs = ProjectDirs::from("dev", "grok-mcp", "grok-mcp")?;
    Some(dirs.config_dir().join("config.toml"))
}

#[derive(Debug, Default)]
struct EnvConfig {
    cookie: Option<String>,
    base_url: Option<String>,
    timeout: Option<Duration>,
    defaults: EnvDefaults,
}

#[derive(Debug, Default)]
struct EnvDefaults {
    mode: Option<Mode>,
    disable_search: Option<bool>,
    force_concise: Option<bool>,
    disable_memory: Option<bool>,
}

impl EnvDefaults {
    fn apply_to(self, target: &mut ChatDefaults) {
        if let Some(mode) = self.mode {
            target.mode = mode;
        }
        if let Some(flag) = self.disable_search {
            target.disable_search = flag;
        }
        if let Some(flag) = self.force_concise {
            target.force_concise = flag;
        }
        if let Some(flag) = self.disable_memory {
            target.disable_memory = flag;
        }
    }
}

impl EnvConfig {
    fn from_env() -> Result<Self> {
        let cookie = env::var("GROK_COOKIE").ok();
        let base_url = env::var("GROK_BASE_URL").ok();
        let timeout = match env::var("GROK_TIMEOUT_SECONDS") {
            Ok(raw) => Some(Duration::from_secs(parse_env_u64(
                "GROK_TIMEOUT_SECONDS",
                &raw,
            )?)),
            Err(_) => None,
        };

        let mode = match env::var("GROK_MODE") {
            Ok(raw) => Some(parse_env_mode(&raw)),
            Err(_) => None,
        };
        let disable_search = parse_optional_bool("GROK_DISABLE_SEARCH")?;
        let force_concise = parse_optional_bool("GROK_FORCE_CONCISE")?;
        let disable_memory = parse_optional_bool("GROK_DISABLE_MEMORY")?;

        Ok(Self {
            cookie,
            base_url,
            timeout,
            defaults: EnvDefaults {
                mode,
                disable_search,
                force_concise,
                disable_memory,
            },
        })
    }
}

fn parse_optional_bool(name: &'static str) -> Result<Option<bool>> {
    match env::var(name) {
        Ok(raw) => Ok(Some(parse_env_bool(name, &raw)?)),
        Err(_) => Ok(None),
    }
}

fn parse_env_bool(name: &'static str, raw: &str) -> Result<bool> {
    match raw.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(Error::Config(ConfigError::InvalidEnv {
            name,
            value: raw.to_owned(),
        })),
    }
}

fn parse_env_u64(name: &'static str, raw: &str) -> Result<u64> {
    raw.trim().parse::<u64>().map_err(|_| {
        Error::Config(ConfigError::InvalidEnv {
            name,
            value: raw.to_owned(),
        })
    })
}

fn parse_env_mode(raw: &str) -> Mode {
    match raw.trim().to_lowercase().as_str() {
        "auto" => Mode::Auto,
        "expert" => Mode::Expert,
        "fast" => Mode::Fast,
        _ => Mode::Other(raw.trim().to_owned()),
    }
}
