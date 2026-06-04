//! Binary entry point: CLI parsing + subcommand dispatch.
//!
//! The default (no-subcommand) action is [`Command::Serve`], which runs the
//! MCP stdio server. Administrative subcommands live behind explicit verbs so
//! future additions don't reshape the default invocation.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use grok_mcp::{
    Config, GrokClient, Server,
    config::{RuntimeState, default_config_path},
};
use rmcp::{ServiceExt, transport::stdio};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "grok-mcp",
    version,
    about = "MCP stdio server bridging agents to grok.com via browser-cookie auth."
)]
struct Cli {
    /// Path to the TOML config file. Defaults to the platform config dir
    /// (`~/.config/grok-mcp/config.toml` on Linux) or `GROK_MCP_CONFIG` if set.
    #[arg(short = 'c', long = "config", env = "GROK_MCP_CONFIG", global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the MCP stdio server (default when no subcommand is given).
    Serve,
    /// Write a starter config TOML with commented defaults.
    InitConfig {
        /// Overwrite an existing config file.
        #[arg(long)]
        force: bool,
    },
    /// Resolve the effective config and print it (cookie redacted).
    PrintConfig,
    /// Verify grok.com cookies by calling `/rest/subscriptions`.
    CheckAuth,
    /// Print each MCP tool name and description, one per line.
    ListTools,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    if let Err(error) = run(cli).await {
        eprintln!("grok-mcp: {error:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let command = cli.command.unwrap_or(Command::Serve);
    let config_override = cli.config.as_deref();

    match command {
        Command::Serve => run_serve(config_override).await,
        Command::InitConfig { force } => run_init_config(config_override, force),
        Command::PrintConfig => run_print_config(config_override),
        Command::CheckAuth => run_check_auth(config_override).await,
        Command::ListTools => run_list_tools(config_override),
    }
}

async fn run_serve(config_override: Option<&Path>) -> anyhow::Result<()> {
    init_tracing();
    let config = Config::load_with_override(config_override).context("loading config")?;
    let Config {
        cookie,
        defaults,
        network,
        challenge,
    } = config;

    let runtime = RuntimeState::new(defaults);
    let client = GrokClient::with_challenge(cookie, network, challenge, runtime.clone())
        .context("building http client")?;
    let server = Server::new(client, runtime);

    let cancel_token = CancellationToken::new();
    let shutdown = cancel_token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            shutdown.cancel();
        }
    });

    tracing::info!("grok-mcp stdio server starting");
    let running = server
        .serve_with_ct(stdio(), cancel_token)
        .await
        .context("starting stdio transport")?;
    let reason = running.waiting().await.context("waiting on stdio loop")?;
    tracing::info!(?reason, "grok-mcp stdio server stopped");
    Ok(())
}

fn run_init_config(config_override: Option<&Path>, force: bool) -> anyhow::Result<()> {
    let target = match config_override {
        Some(explicit) => explicit.to_path_buf(),
        None => {
            default_config_path().context("no default config path (HOME / XDG dirs unavailable)")?
        }
    };

    if target.exists() && !force {
        bail!(
            "config already exists at {path} (pass --force to overwrite)",
            path = target.display()
        );
    }

    if let Some(parent) = target.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("creating config directory {path}", path = parent.display())
        })?;
    }

    std::fs::write(&target, CONFIG_TEMPLATE)
        .with_context(|| format!("writing config template to {path}", path = target.display()))?;

    println!("wrote starter config to {path}", path = target.display());
    println!("edit it, drop in your grok.com cookie header, and re-run `grok-mcp`.");
    Ok(())
}

fn run_print_config(config_override: Option<&Path>) -> anyhow::Result<()> {
    let config = Config::load_with_override(config_override).context("loading config")?;
    let Config {
        cookie: _,
        defaults,
        network,
        challenge: _,
    } = config;

    println!("# grok-mcp effective config");
    println!("cookie = \"<redacted>\"");
    println!();
    println!("[defaults]");
    println!("mode = \"{mode}\"", mode = defaults.mode.as_wire());
    println!("disable_search = {}", defaults.disable_search);
    println!("force_concise = {}", defaults.force_concise);
    println!("disable_memory = {}", defaults.disable_memory);
    println!(
        "enable_image_generation = {}",
        defaults.enable_image_generation
    );
    println!(
        "image_generation_count = {}",
        defaults.image_generation_count
    );
    println!("enable_side_by_side = {}", defaults.enable_side_by_side);
    println!(
        "disable_text_follow_ups = {}",
        defaults.disable_text_follow_ups
    );
    println!();
    println!("[network]");
    println!("base_url = {}", toml_string(&network.base_url)?);
    println!("user_agent = {}", toml_string(&network.user_agent)?);
    println!(
        "timeout_seconds = {seconds}",
        seconds = network.timeout.as_secs()
    );
    println!(
        "stream_idle_timeout_seconds = {seconds}",
        seconds = network.stream_idle_timeout.as_secs()
    );
    Ok(())
}

async fn run_check_auth(config_override: Option<&Path>) -> anyhow::Result<()> {
    init_tracing();
    let config = Config::load_with_override(config_override).context("loading config")?;
    let Config {
        cookie,
        defaults,
        network,
        challenge,
    } = config;
    let runtime = RuntimeState::new(defaults);
    let client = GrokClient::with_challenge(cookie, network, challenge, runtime)
        .context("building http client")?;

    let subscriptions = client
        .subscriptions()
        .await
        .context("calling /rest/subscriptions")?;

    let Some(sub) = subscriptions.primary() else {
        bail!("authenticated, but grok.com returned no subscriptions for this account");
    };

    println!("authenticated: yes");
    println!("user_id: {user}", user = sub.xai_user_id.as_str());
    println!("tier: {tier:?}", tier = sub.tier);
    println!("status: {status:?}", status = sub.status);
    if let Some(stripe) = sub.active_until() {
        println!("active_until: {stripe}");
    }
    if subscriptions.subscriptions.len() > 1 {
        println!("subscriptions:");
        for subscription in &subscriptions.subscriptions {
            print!(
                "  - user_id: {user}; tier: {tier:?}; status: {status:?}",
                user = subscription.xai_user_id.as_str(),
                tier = subscription.tier,
                status = subscription.status,
            );
            if let Some(active_until) = subscription.active_until() {
                print!("; active_until: {active_until}");
            }
            println!();
        }
    }
    Ok(())
}

fn toml_string(raw: &str) -> anyhow::Result<String> {
    serde_json::to_string(raw).context("encoding string for TOML output")
}

fn run_list_tools(config_override: Option<&Path>) -> anyhow::Result<()> {
    // Building a Server requires a cookie; keep the config load so the
    // subcommand exercises the same path. Users who haven't configured yet
    // get a useful error instead of a silent empty list.
    let config = Config::load_with_override(config_override).context("loading config")?;
    let Config {
        cookie,
        defaults,
        network,
        challenge,
    } = config;
    let runtime = RuntimeState::new(defaults);
    let client = GrokClient::with_challenge(cookie, network, challenge, runtime.clone())
        .context("building http client")?;
    let server = Server::new(client, runtime);

    for tool in server.tools() {
        let description = tool
            .description
            .as_deref()
            .unwrap_or("(no description)")
            .replace('\n', " ");
        println!("{name}\t{description}", name = tool.name);
    }
    Ok(())
}

/// `tracing-subscriber` writes to **stderr only**. stdout is reserved for the
/// MCP wire protocol — any accidental write to stdout corrupts it.
fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_target(false)
        .init();
}

const CONFIG_TEMPLATE: &str = r#"# grok-mcp starter config.
#
# Paste your grok.com cookie header here. Required cookies: sso + sso-rw.
# Optional but recommended: x-userid, cf_clearance, __cf_bm.
# Grab from the browser: DevTools → Application → Cookies → https://grok.com.
#
# Alternatively, set GROK_COOKIE in the environment and leave this file as-is.
# cookie = "sso=...; sso-rw=...; x-userid=...; cf_clearance=...; __cf_bm=..."

[defaults]
# "auto" | "expert" | "fast" | any other string — non-enum values pass through.
# Default is "expert" to match the grok_research hero tool.
mode = "expert"
disable_search = false
force_concise = false
disable_memory = false
enable_image_generation = true
image_generation_count = 2
enable_side_by_side = true
disable_text_follow_ups = false

[network]
base_url = "https://grok.com"
# "<auto>" (case-insensitive) keeps the built-in Chrome-like default.
user_agent = "<auto>"
timeout_seconds = 120
stream_idle_timeout_seconds = 60
"#;
