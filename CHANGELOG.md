# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `ChatOptions` and `IntegrationFlags` in `models::options` — user-facing
  per-call chat overrides, exported from the crate root.
- `PollStatus` enum replacing the stringly-typed `PollOutput.status` field.

### Changed
- BREAKING (library API, not wire): `Conversations::start(message)` now takes
  the initial message as a positional argument, so new-conversation requests
  require it at compile time.
- BREAKING (library API): `Conversations::continue_` and `Conversations::get`
  now accept `impl Into<ConversationId>`, so `&str`, `String`, and
  `ConversationId` all work directly.
- `StartConversation` and `ContinueConversation` now share the single
  `ChatOptions` struct instead of duplicating the per-call mode, attachment,
  and integration-flag fields.
- `PollOutput.status` now uses the `PollStatus` enum while preserving the JSON
  wire values (`"in_progress"`, `"completed"`, `"not_found"`).

### Removed
- `apply_research_options!` and the private conversation-builder override
  helpers, now replaced by `ChatOptions` conversions.

### Fixed
- `grok_research_poll` no longer reports `status: "completed"` with an empty
  `message` while Grok is still generating. Readiness now requires actual
  content (non-empty `message` or at least one step) in the loaded response:
  grok.com stamps `partial: false` on the response record at creation time,
  before any content exists, so `partial: false` alone is not a reliable
  readiness signal. `partial: true` still blocks readiness as before.

## [0.1.0] — 2026-04-18

### Added
- Stdio MCP server bridging local agents to grok.com via browser-cookie auth.
- 11 MCP tools: `grok_check_auth`, `grok_rate_limits`, `grok_research`,
  `grok_research_start`, `grok_research_poll`, `grok_list_conversations`,
  `grok_get_conversation`, `grok_upload_file`, `grok_list_skills`,
  `grok_get_defaults`, `grok_set_defaults`.
- `grok_research` — deep research via Grok with web search, expert reasoning,
  and tool use. Defaults to `Mode::Expert`. Optional `full_details` post-hydrates
  the response with typed `steps[]` (tags, rolloutId, toolUsageCards,
  toolUsageResults, webSearchResults).
- `grok_research_start` / `grok_research_poll` — async pattern for long expert
  queries that exceed MCP client timeouts. Stateless, crash-safe.
- Typed `citations` extracted from inline `<grok:render>` XML + `cardAttachment`
  stream frames. Typed `tool_usage_cards` from `<xai:tool_usage_card>` XML in
  thinking tokens, deduplicated against structured frames. Typed `agent_messages`
  from `chatroomSend` in multi-agent expert runs.
- Per-call connector toggles: `enable_gmail_search`,
  `enable_google_calendar_search`, `enable_outlook_search`,
  `enable_outlook_calendar_search`, `enable_google_drive_search`.
- `grok_get_conversation` returns typed `Conversation`, `message_count`, and
  `total_chars`.
- Cloudflare-passing headers: Chrome-like User-Agent, `sec-ch-ua*`,
  `sec-fetch-*`, per-request `x-statsig-id` and `x-xai-request-id`.
- Normalized `StreamEvent` enum flattening both observed NDJSON envelope shapes.
- Layered config: defaults → TOML file → env vars → runtime overrides.
- Cookie validation: `sso` + `sso-rw` required, others warn-only.
- Progress notification forwarding with heartbeat every ~10s of silence.
- NDJSON idle-timeout enforcement per-chunk.
- CLI: `serve` (default), `init-config`, `print-config`, `check-auth`,
  `list-tools` with global `-c/--config`.

### Changed
- BREAKING: `GrokClient` API redesigned as resource-oriented modules with
  fluent consuming-self builders backed by `bon`. Complex positional methods
  are replaced by drill-down resources:
  - `client.conversations().start().message(msg).mode(mode).send()`
  - `client.conversations().continue_(id).message(msg).send()`
  - `client.conversations().list().page_size(50).send()`
  - `client.conversations().get(id).include_messages(true).send()`
  - `client.uploads().upload().file_name(name).content_base64(data).send()`
- Simple endpoints remain direct methods on `GrokClient`:
  `subscriptions`, `rate_limits`, `skills`, and `get_asset`.
- `grok_research` description now warns that heavy expert queries can exceed
  MCP client timeouts and points callers to `grok_research_start` +
  `grok_research_poll`.

### Fixed
- `grok_get_conversation` with `include_messages=false` no longer returns a
  schema validation error for omitted optional response fields.
- `grok_get_conversation` no longer fails with `missing field
  "conversationId"`; the `/conversations_v2/<id>` envelope is unwrapped before
  returning the typed `Conversation`.

### Security
- Cookie wrapped in `secrecy::SecretString`, `Debug` prints `<redacted>`.
- `tracing-subscriber` writes to stderr only (stdout reserved for MCP wire).

[Unreleased]: https://github.com/Hirasawa69/grok-mcp/compare/0.1.0...HEAD
[0.1.0]: https://github.com/Hirasawa69/grok-mcp/releases/tag/0.1.0
