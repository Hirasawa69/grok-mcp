//! # grok-mcp
//!
//! A Model Context Protocol server that bridges local agents to grok.com via
//! browser-session cookie authentication. Install the binary and wire it up as
//! a stdio MCP server in your client (Claude Desktop, opencode, Cursor, …) to
//! give agents access to Grok chat, web search, memory, and file uploads using
//! the same session you already have in a browser.
//!
//! ## Scope
//!
//! This crate is split into a reusable library surface (HTTP client + typed
//! domain models) and a thin binary (`main.rs`) that wires it into an MCP stdio
//! server. The library pieces are:
//!
//! * [`Config`] — layered configuration (TOML file + env vars).
//! * [`GrokClient`] — REST + streaming NDJSON bindings for `grok.com/rest/*`.
//! * [`Server`] — MCP tool router that exposes the client to agents.
//! * [`models`] — typed request/response shapes, newtype IDs, the normalized
//!   stream-event enum.
//! * [`cookie::GrokCookie`] — validated, redacted-on-`Debug` cookie header.
//! * [`error::Error`] — library-layer typed error; converts to `rmcp::ErrorData`
//!   at the MCP boundary.
//!
//! ## Security
//!
//! The cookie header is wrapped in [`secrecy::SecretString`] and never logged.
//! `tracing` output is directed to **stderr only** — stdout is reserved for the
//! MCP wire protocol.
//!
//! ## Stability
//!
//! grok.com's `/rest/app-chat/*` surface is an undocumented internal web-client
//! API. The endpoints covered by this crate are derived from observed grok.com
//! traffic and can change whenever grok.com updates the web client. This crate
//! tracks the read/chat/upload surface that has been validated against observed
//! requests; removed or failing endpoints are dropped instead of kept as stubs.

pub mod client;
pub mod config;
pub mod cookie;
pub mod error;
pub mod models;
pub mod server;

pub use client::GrokClient;
pub use config::Config;
pub use cookie::GrokCookie;
pub use error::{Error, Result};
pub use server::Server;
