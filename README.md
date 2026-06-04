# grok-mcp

A [Model Context Protocol](https://modelcontextprotocol.io) stdio server that
bridges local agents to [grok.com](https://grok.com) using your existing
browser session (via cookie header). Install once, register as a local MCP
server in your client, and your agent gets access to Grok chat, web search,
memory, skills, file uploads, and conversation history — using the same
session you already have open in a browser.

> ⚠️ **This bridges an undocumented internal web-client API.** Everything here
> is derived from observed grok.com traffic. Endpoints can (and will) break
> when grok.com changes.
> For programmatic production access, use xAI's official
> [`api.x.ai`](https://x.ai/api) instead — it is a separate product and out of
> scope for this project.

## Why cookie auth (and not the official API)

`api.x.ai` is a paid per-token product. It does **not** give you access to
your personal grok.com conversation history, persistent memory, the built-in
web-search tool, file attachments, or the agent/expert-mode reasoning trace.
This project exposes those browser-session-only read/chat features to a local
agent, which is something only the web client can do today.

## Install

Requires Rust 1.84 or later.

```bash
cargo install --git https://github.com/Hirasawa69/grok-mcp --locked
```

The binary is published as `grok-mcp`. Confirm the install:

```bash
grok-mcp --version
```

## Configure

### 1. Grab your grok.com cookie

1. Open [grok.com](https://grok.com) in a browser. Make sure you're logged in.
2. Open DevTools → **Application** → **Cookies** → `https://grok.com`.
3. Copy every cookie pair into a single header string:
   ```
   sso=...; sso-rw=...; x-userid=...; cf_clearance=...; __cf_bm=...
   ```
   The minimum required pair is `sso` + `sso-rw` — the server will fail to
   start if either is missing. `x-userid`, `cf_clearance`, and `__cf_bm`
   trigger a warning but are not strictly required.

The cookie header is held in memory only. It is wrapped in a `SecretString`,
never written to logs, and never sent to stdout.

### 2. Configure via TOML or env

Either drop a TOML file at the platform config dir
(`~/.config/grok-mcp/config.toml` on Linux):

```toml
cookie = "sso=...; sso-rw=...; x-userid=...; cf_clearance=...; __cf_bm=..."

[defaults]
mode = "expert"             # "auto" | "expert" | "fast" | any other string
disable_search = false
force_concise = false
disable_memory = false
enable_image_generation = false
image_generation_count = 0
enable_side_by_side = false
disable_text_follow_ups = false

[network]
base_url = "https://grok.com"
user_agent = "<auto>"        # use the Chrome-like default
timeout_seconds = 120
stream_idle_timeout_seconds = 60
```

…or set individual environment variables:

| Variable               | Maps to                   | Notes                             |
|------------------------|---------------------------|-----------------------------------|
| `GROK_MCP_CONFIG`      | path to TOML              | overrides default config location |
| `GROK_COOKIE`          | `cookie`                  | full cookie header string         |
| `GROK_MODE`            | `defaults.mode`           | `auto`/`expert`/`fast`/any string |
| `GROK_DISABLE_SEARCH`  | `defaults.disable_search` | `1`/`true`/`0`/`false`            |
| `GROK_FORCE_CONCISE`   | `defaults.force_concise`  | —                                 |
| `GROK_DISABLE_MEMORY`  | `defaults.disable_memory` | —                                 |
| `GROK_BASE_URL`        | `network.base_url`        | for local testing                 |
| `GROK_TIMEOUT_SECONDS` | `network.timeout_seconds` | overall request timeout           |
| `RUST_LOG`             | tracing filter            | writes to **stderr** only         |

Precedence, lowest → highest: defaults → TOML file → env vars → runtime
mutations via the `grok_set_defaults` tool (not persisted).

## CLI

```
grok-mcp [-c|--config <PATH>] [COMMAND]

Commands:
  serve         Run the MCP stdio server (default when no subcommand is given)
  init-config   Write a starter config TOML with commented defaults
                  --force      overwrite an existing file
  print-config  Resolve the effective config and print it (cookie redacted)
  check-auth    Call /rest/subscriptions and report tier + user id
  list-tools    Print each MCP tool name + description, tab-separated
```

`-c/--config` (or `GROK_MCP_CONFIG`) is global and accepted on every
subcommand; passing an explicit path that does not exist is an error.

Typical first-run workflow:

```bash
grok-mcp init-config             # drop a starter config at the default path
nano ~/.config/grok-mcp/config.toml   # paste your cookie header
grok-mcp check-auth              # verify cookies, print your tier
grok-mcp                         # start the stdio server (== grok-mcp serve)
```

## Register with an MCP client

### Opencode

```json
{
  "mcp": {
    "grok": {
      "type": "local",
      "command": [
        "grok-mcp"
      ],
      "enabled": true
    }
  }
}
```

## Tools

All tools return structured JSON. Names are prefixed with `grok_`.

| Tool                      | Description                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
|---------------------------|--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `grok_research`           | **Hero tool.** Deep research via Grok in a single call. Streams the answer and blocks until complete. Use this only for non-expert modes or expert queries you expect to finish inside your MCP client's timeout. Heavy expert queries should use `grok_research_start` + `grok_research_poll` instead: if `grok_research` times out client-side, the answer cannot be recovered. Pass `verbosity="minimal"` to keep only the final answer + citations, `verbosity="standard"` (default) for today's streamed payload, or `verbosity="full"` to hydrate structured `steps[]` plus `agent_messages[]` with one extra HTTP round-trip. `full_details=true` still works as a deprecated alias for `verbosity="full"`. Also accepts per-call connector overrides: `enable_gmail_search`, `enable_google_calendar_search`, `enable_outlook_search`, `enable_outlook_calendar_search`, and `enable_google_drive_search`. |
| `grok_research_start`     | Begin a long-running Grok research request without waiting for the answer. Returns `conversation_id` + `response_id` immediately; use `grok_research_poll` later to retrieve the result. Prefer this for any expert-mode query or whenever you suspect the run may take more than ~60 seconds, because the job continues on Grok's side even if your client would otherwise time out.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `grok_research_poll`      | Poll a previously started research result by (`conversation_id`, `response_id`). Returns `status: "completed"` with the full result when ready, `status: "in_progress"` while Grok is still generating, or `status: "not_found"` if the id pair is unknown. Pass `verbosity="minimal"`, `"standard"`, or `"full"` to control payload size. `include_thinking=true` adds per-step thinking traces separately from the hydrated `verbosity="full"` fields.                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `grok_check_auth`         | Check that grok.com cookies still work. Returns the logged-in user id, subscription tier, and status.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| `grok_rate_limits`        | Remaining query budget (per-mode rate limit). Defaults to the current runtime mode.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `grok_list_conversations` | List grok.com conversations. Pass `page_token` from a prior response to paginate.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `grok_get_conversation`   | Fetch a conversation. Set `include_messages` to also load the full message thread.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `grok_upload_file`        | Upload a file and get back an attachment id to pass into `grok_research`. Provide `content_base64` or `local_path`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `grok_list_skills`        | List grok.com built-in skills (catalog of tools Grok itself can invoke).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| `grok_get_defaults`       | Read the current runtime defaults (mode + flags).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| `grok_set_defaults`       | Override runtime defaults in-memory (not persisted). Only fields you pass are changed.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             |

`grok_research` defaults to `mode = "expert"` when the caller doesn't supply
one, regardless of the runtime-default mode. The runtime default still
governs the other tools (e.g. `grok_rate_limits`).

### Async research pattern

Heavy expert-mode queries can take 3–10 minutes. If your MCP client times out
before the answer arrives, use the two-step async pattern:

1. `grok_research_start` — fires off the query and returns
   `conversation_id` + `response_id` immediately
2. `grok_research_poll` — checks whether the answer is ready. It returns
    `status: "completed"` with the full result when done, or
    `status: "in_progress"` if Grok is still working

The sync `grok_research` tool remains available for quick queries that fit
within your client's timeout.

Use `verbosity="minimal"` on either `grok_research` or `grok_research_poll`
when you only need the final answer and citations. This trims large expert
payload fields like `thinking`, `web_search_results`, `tool_usage_cards`, and
hydrated `steps[]` out of the MCP response. `verbosity="standard"` preserves
the current default shape, while `verbosity="full"` adds hydrated `steps[]`
plus `agent_messages[]`. The older `full_details=true` flag remains accepted as
an alias for `verbosity="full"`.

> **Note**: `grok_research_poll` cannot return the top-level `thinking` trace
> because that data exists only on the live stream. Use `verbosity="full"`
> (or the deprecated `full_details=true` alias) to get structured `steps[]`
> instead.

### Progress streaming

If the MCP caller sets a `progressToken` in request metadata, `grok_research`
forwards Grok's token stream as progress notifications. Tokens tagged
`header`/`summary`/`final` are concatenated and flushed on a short timer so
the client can render incrementally, and discrete updates are also emitted for
tool cards, web-search batches, the echoed user turn, a final `answering`
marker, and a heartbeat roughly every 10 seconds of silence.

## Security notes

* The cookie header is held in a [`secrecy::SecretString`] and never appears
  in any log or `Debug` output.
* `tracing` writes to **stderr only**. stdout is reserved for the MCP wire
  protocol; anything written there would corrupt the JSON-RPC stream.
* No state is persisted to disk. Runtime overrides via `grok_set_defaults`
  live only for the lifetime of the server process.

`Mode` is deliberately non-exhaustive: `auto` and `expert` are confirmed from
observed traffic, `fast` is a rumoured value, and any other string passes
through via `Mode::Other(String)` so you can experiment without an enum bump.

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this work by you, as defined in the
Apache-2.0 license, shall be dual-licensed as above, without any additional
terms or conditions.
