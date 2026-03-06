# Design Document: Chat Bot Ingestion Triggers

**Author:** Scott Idler
**Date:** 2026-03-06
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Add Telegram and Discord bot integrations to obsidian-borg as config-driven ingestion triggers. When a user shares a URL to a private Telegram bot or Discord channel, the daemon extracts the URL, runs it through the existing `pipeline::process_url()`, and replies with the result. Bot integrations are entirely optional — defined by the presence of config sections. No config section, no bot task spawned. The dependencies compile in unconditionally (they share most of their transitive deps with the existing crate), but no connections are made and no resources are consumed unless the config enables them.

## Problem Statement

### Background

obsidian-borg currently accepts URLs via a single channel: HTTP POST to `/ingest`. This works well with HTTP Shortcuts on Android and curl/bookmarklets on desktop, but requires the user to configure a Share target app or remember a hotkey. The original ideas document (Paradigm 1) proposed using a messaging app as the ingestion gateway because the Share Menu is the most universal cross-platform UI pattern that already exists.

### Problem

The HTTP-only model has friction:
- **Mobile:** HTTP Shortcuts works but is a niche app. Every phone already has Telegram/Discord installed and in the Share menu.
- **Cross-device queuing:** If the daemon is down, HTTP Shortcuts silently fails. Telegram/Discord queue messages in chat history and the bot picks them up when it reconnects.
- **Feedback loop:** HTTP Shortcuts can show raw JSON but it's ugly. A chat reply like "Saved: Deep Dive into LLMs.md (#ai, #tech)" is natural.
- **Discoverability:** Sharing to a chat app is muscle memory. Sharing to a custom HTTP target is not.

### Goals

- Telegram bot integration: long-poll for messages, extract URLs, ingest, reply with result
- Discord bot integration: listen on a designated channel, same flow
- Purely config-driven: add a `telegram` or `discord` section to the YAML → bot starts. Remove it → bot doesn't start. No recompilation.
- Bots are just triggers — they feed the same `pipeline::process_url()` as the HTTP endpoint
- Graceful shutdown: all bot tasks stop cleanly when the daemon receives SIGTERM/SIGINT
- Startup logging: clearly indicate which integrations are active

### Non-Goals

- WhatsApp integration (no stable bot API, requires Meta business verification, breaks regularly)
- Slash commands, interactive menus, or rich bot features beyond URL ingestion and result replies
- Webhook mode for Telegram (long polling is simpler, needs no public IP, works behind Tailscale)
- Bot-initiated messages (the bot only responds, never initiates)
- Multi-user access control (this is a personal tool; the bot token is the auth boundary)
- Telegram group chat support (bot is intended for DM use; group chats require privacy mode configuration)
- Discord DM support (bot only listens on the configured guild channel)

## Proposed Solution

### Overview

Add two optional modules (`telegram.rs`, `discord.rs`) that each implement a long-running async task. At startup, `run_server()` inspects the config: if `config.telegram` is `Some`, spawn the Telegram task; if `config.discord` is `Some`, spawn the Discord task. The HTTP server always runs. All tasks share an `Arc<Config>` and call the same pipeline.

### Architecture

```
                    ┌───────────────────────────────────────┐
                    │            obsidian-borg              │
                    │                                       │
  HTTP POST ──────▶ │  axum /ingest ──┐                     │
                    │                 │                     │
  Telegram msg ──▶ │  telegram.rs ───┤── pipeline::       │
                    │                 │   process_url() ──▶ vault/Inbox/
  Discord msg ───▶ │  discord.rs ────┘                     │
                    │                                       │
                    └───────────────────────────────────────┘
```

All three ingestion paths converge on `pipeline::process_url(url, tags, config)`. The bot modules are thin adapters: receive message → extract URL → call pipeline → format reply → send reply.

### Config Model

```yaml
# Optional: omit entirely to disable
telegram:
  bot_token_env: "TELEGRAM_BOT_TOKEN"   # env var containing the token
  allowed_chat_ids: []                   # empty = allow all, or list specific chat IDs

# Optional: omit entirely to disable
discord:
  bot_token_env: "DISCORD_BOT_TOKEN"    # env var containing the token
  channel_id: 1234567890                 # only listen on this channel
```

Config structs use `Option<T>` — serde deserializes a missing section as `None`:

```rust
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub vault: VaultConfig,
    pub transcriber: TranscriberConfig,
    pub groq: GroqConfig,
    pub llm: LlmConfig,
    pub telegram: Option<TelegramConfig>,
    pub discord: Option<DiscordConfig>,
    pub debug: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramConfig {
    pub bot_token_env: String,
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,  // empty = allow all
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscordConfig {
    pub bot_token_env: String,
    pub channel_id: u64,
}
```

Token indirection via `bot_token_env` keeps secrets out of the YAML file (same pattern as `groq.api_key_env`).

### Startup Flow

`run_server()` changes from blocking on `axum::serve()` to managing all tasks via a `JoinSet`:

```rust
pub async fn run_server(config: Config, _verbose: bool) -> Result<()> {
    let config = Arc::new(config);
    let mut tasks = tokio::task::JoinSet::new();

    // HTTP server (always runs)
    let app = build_router(config.clone());
    let listener = TcpListener::bind(addr).await?;
    tasks.spawn(async move {
        axum::serve(listener, app).await.map_err(|e| eyre::eyre!(e))
    });
    println!("{} http server on {addr}", "-->".green());

    // Telegram bot (config-driven)
    if let Some(tg_config) = &config.telegram {
        let token = std::env::var(&tg_config.bot_token_env)
            .context("Telegram bot token env var not set")?;
        let tg = tg_config.clone();
        let cfg = config.clone();
        tasks.spawn(async move { telegram::run(token, tg, cfg).await });
        println!("{} telegram bot active", "-->".green());
    }

    // Discord bot (config-driven)
    if let Some(dc_config) = &config.discord {
        let token = std::env::var(&dc_config.bot_token_env)
            .context("Discord bot token env var not set")?;
        let dc = dc_config.clone();
        let cfg = config.clone();
        tasks.spawn(async move { discord::run(token, dc, cfg).await });
        println!("{} discord bot active", "-->".green());
    }

    // If any task exits (error or completion), propagate
    if let Some(result) = tasks.join_next().await {
        result??;
    }

    Ok(())
}
```

`JoinSet` is cleaner than `tokio::select!` with `Option<JoinHandle>` — no need for `std::future::pending()` workarounds, and it naturally handles any number of optional tasks.

### Module Design

#### `telegram.rs`

Uses the `teloxide` crate (v0.17, the standard Rust Telegram framework). Long-polls the Telegram Bot API via `getUpdates`.

Note: `teloxide::repl()` is convenient but owns its own shutdown flow. We use the lower-level `Dispatcher` API with a `CancellationToken` so the task can be shut down cleanly when the process receives SIGTERM.

```rust
pub async fn run(token: String, tg_config: TelegramConfig, config: Arc<Config>) -> Result<()> {
    let bot = teloxide::Bot::new(&token);

    let handler = Update::filter_message().endpoint(
        move |message: Message, bot: Bot| {
            let config = config.clone();
            let allowed = tg_config.allowed_chat_ids.clone();
            async move {
                // Access control
                if !allowed.is_empty() && !allowed.contains(&message.chat.id.0) {
                    return Ok(());
                }

                // Extract URL from message text
                let text = message.text().unwrap_or("");
                let Some(url) = extract_url_from_text(text) else {
                    bot.send_message(message.chat.id, "No URL found in message.").await?;
                    return Ok(());
                };

                bot.send_message(message.chat.id, "Processing...").await?;

                let result = pipeline::process_url(&url, vec![], &config).await;
                let reply = format_reply(&result, &url);
                bot.send_message(message.chat.id, reply).await?;

                Ok(())
            }
        },
    );

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
```

#### `discord.rs`

Uses the `serenity` crate (v0.12, the standard Rust Discord library). Connects to the Discord Gateway via WebSocket and listens for messages in the configured channel.

```rust
pub async fn run(token: String, dc_config: DiscordConfig, config: Arc<Config>) -> Result<()> {
    let handler = Handler { config, channel_id: dc_config.channel_id };
    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

    let mut client = serenity::Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .context("Failed to create Discord client")?;

    client.start().await.context("Discord client error")?;
    Ok(())
}

struct Handler { config: Arc<Config>, channel_id: u64 }

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: Context, msg: Message) {
        if msg.channel_id.get() != self.channel_id || msg.author.bot { return; }

        let Some(url) = extract_url_from_text(&msg.content) else { return; };

        let _ = msg.channel_id.say(&ctx.http, "Processing...").await;
        let result = pipeline::process_url(&url, vec![], &self.config).await;
        let _ = msg.channel_id.say(&ctx.http, format_reply(&result, &url)).await;
    }
}
```

#### URL Extraction

Both modules share a URL extraction utility. People don't always send bare URLs — they might send "check this out https://youtube.com/watch?v=abc" or Telegram may wrap URLs in entities. The extractor handles:

1. Telegram message entities (type `Url`) — structured, reliable
2. Fallback: regex scan for `https?://` patterns in the message text

```rust
// Shared utility in url_router.rs alongside classify_url()
use std::sync::LazyLock;

static URL_REGEX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"https?://\S+").expect("valid regex"));

pub fn extract_url_from_text(text: &str) -> Option<String> {
    URL_REGEX.find(text).map(|m| {
        // Strip common trailing punctuation that gets captured from prose
        m.as_str().trim_end_matches(['.', ',', ')', ']', '>', ';', '!']).to_string()
    })
}
```

The regex is compiled once via `LazyLock` (stable in Rust 1.80+), not per invocation.

#### Reply Formatting

Both bots share a `format_reply()` function that lives in `url_router.rs` alongside `extract_url_from_text()` (both are shared utilities used by bot modules). It converts `IngestResult` into a human-friendly chat message:

```rust
pub fn format_reply(result: &IngestResult, url: &str) -> String {
    match &result.status {
        IngestStatus::Completed => {
            let title = result.title.as_deref().unwrap_or("Untitled");
            let tags = if result.tags.is_empty() {
                String::new()
            } else {
                format!("\nTags: {}", result.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(", "))
            };
            format!("Saved: {title}{tags}")
        }
        IngestStatus::Failed { reason } => {
            format!("Failed: {reason}\nURL: {url}")
        }
        IngestStatus::Queued => "Queued for processing.".to_string(),
    }
}
```

Example outputs:
```
Saved: Deep Dive into LLMs
Tags: #ai, #tech
```

```
Failed: network error
URL: https://example.com/broken
```

### Data Model

No changes to existing types. `IngestResult` already contains everything needed for the reply (`status`, `title`, `note_path`, `tags`).

### API Design

No changes to the HTTP API. The bots are internal consumers of `pipeline::process_url()`, not HTTP clients.

### Implementation Plan

#### Phase 1: Config and Startup Scaffolding

- Add `Option<TelegramConfig>` and `Option<DiscordConfig>` to `Config`
- Add `teloxide` and `serenity` to `Cargo.toml`
- Add `regex` to `Cargo.toml` (for URL extraction)
- Refactor `run_server()` to use `JoinSet` with optional spawned tasks
- Add startup logging for active integrations
- Tests: config deserialization with and without bot sections

#### Phase 2: URL Extraction and Reply Formatting

- Add `extract_url_from_text()` utility function
- Add `format_reply()` function for chat-friendly IngestResult rendering
- Tests: URL extraction from bare URLs, URLs in sentences, URLs with trailing punctuation, no-URL messages
- Tests: reply formatting for success and failure cases

#### Phase 3: Telegram Integration

- Add `telegram.rs` module
- Implement `telegram::run()` using teloxide long-poll
- Handle message entities for URL extraction
- Handle `allowed_chat_ids` access control
- Send processing feedback ("Processing...") and result reply
- Tests: unit tests for message handling logic (mocked bot)

#### Phase 4: Discord Integration

- Add `discord.rs` module
- Implement `discord::run()` using serenity event handler
- Filter by `channel_id` and ignore bot messages
- Same pipeline call and reply pattern
- Tests: unit tests for handler logic

#### Phase 5: Documentation and Example Config

- Update `obsidian-borg.example.yml` with commented-out telegram/discord sections
- Update `deploy/README.md` with bot setup instructions
- Document BotFather setup for Telegram
- Document Discord Developer Portal setup

## Alternatives Considered

### Alternative 1: Sidecar Process per Bot

- **Description:** Each bot runs as a separate binary that HTTP POSTs to `/ingest`.
- **Pros:** Zero coupling. Can be written in Python. Each bot is independently deployable.
- **Cons:** Two extra systemd units, two extra processes, config spread across multiple files, no shared pipeline (adds HTTP serialization overhead), harder to keep in sync.
- **Why not chosen:** The config-driven approach is simpler for a personal tool. One binary, one config, one deploy. If the tool later needs to scale to a multi-user SaaS, the sidecar model would be appropriate, but that's not the use case.

### Alternative 2: Telegram Webhook Mode Instead of Long Polling

- **Description:** Configure Telegram to POST updates to a public URL instead of the bot polling for them.
- **Pros:** Slightly more efficient (no idle polling). Standard for production bots at scale.
- **Cons:** Requires a publicly reachable HTTPS endpoint. Behind Tailscale, this means either a Cloudflare tunnel or exposing a port. Adds TLS certificate management. Long polling works fine for a single-user bot with low message volume.
- **Why not chosen:** Long polling is simpler, needs no public IP, and works behind Tailscale/NAT out of the box.

### Alternative 3: WhatsApp Integration

- **Description:** Add WhatsApp as a third bot option.
- **Pros:** WhatsApp is the most used messaging app globally.
- **Cons:** Official Business API requires Meta business verification, costs money per conversation, and requires a dedicated phone number. Unofficial libraries reverse-engineer the web client and break regularly. No stable bot ecosystem.
- **Why not chosen:** The cost/complexity is not justified for a personal tool. If WhatsApp adds a free personal bot API in the future, it can be added as another config-driven module using the same pattern.

### Alternative 4: Generic "Bot" Trait with Dynamic Dispatch

- **Description:** Define a `trait BotIntegration` and use `Vec<Box<dyn BotIntegration>>` for runtime polymorphism.
- **Pros:** Clean abstraction, easy to add new integrations.
- **Cons:** Over-engineering for two integrations. The trait would have one method (`run`), each implementation would be completely different internally (HTTP long-poll vs WebSocket), and the only shared code is URL extraction and reply formatting — which are already shared as utility functions.
- **Why not chosen:** Two concrete modules with shared utility functions is simpler and more readable than a trait hierarchy. If a third integration is added, revisit.

## Technical Considerations

### Dependencies

| Dependency | Purpose | Size Impact |
|-----------|---------|-------------|
| `teloxide` | Telegram Bot API framework | Medium (pulls in reqwest, serde — both already in tree) |
| `serenity` | Discord API library | Medium (adds tokio-tungstenite for WebSocket) |
| `regex` | URL extraction from message text | Small |

Both `teloxide` and `serenity` use `tokio` and `reqwest`, which are already dependencies. The incremental binary size increase is primarily the protocol-specific code.

### Performance

- Telegram long-polling: one idle HTTP connection, negligible CPU/memory
- Discord Gateway: one WebSocket connection, negligible CPU/memory
- Pipeline calls are the same cost regardless of trigger source
- No contention: each bot task has its own async context, pipeline is stateless

### Security

- Bot tokens are read from environment variables, never stored in the config file
- `allowed_chat_ids` provides optional access control for Telegram (only process messages from your own chat)
- Discord `channel_id` filtering ensures the bot only responds in the designated channel
- Discord `MESSAGE_CONTENT` privileged intent must be enabled in the Discord Developer Portal for the bot to read message text. This is documented in Phase 5.
- The bot tokens themselves are the primary auth boundary — anyone with the token can message the bot. For a personal tool behind Tailscale, this is acceptable.

### Testing Strategy

- **Config tests:** Deserialize YAML with/without telegram/discord sections, verify `None`/`Some`
- **URL extraction tests:** Bare URLs, URLs in sentences, multiple URLs (takes first), no URLs, malformed text
- **Reply formatting tests:** Success result → friendly message, failure result → error message
- **Bot module tests:** Unit test message handling logic with mock pipeline calls. Integration testing with real Telegram/Discord APIs is impractical in CI — manual testing during development.
- **Startup tests:** Verify `run_server()` doesn't panic when bot configs are `None`

### Rollout Plan

1. Implement on a feature branch
2. `otto ci` green after each phase
3. Manual testing: create a test Telegram bot, send URLs, verify notes appear in vault
4. Manual testing: create a test Discord server/channel, same flow
5. Merge to main

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `teloxide` API changes break compilation | Low | Low | Pin version in Cargo.toml, update on our schedule |
| `serenity` API changes break compilation | Low | Low | Pin version in Cargo.toml |
| Telegram rate limits on long polling | Very Low | Low | teloxide handles backoff internally |
| Discord Gateway disconnects | Low | Low | serenity auto-reconnects |
| Bot token leaked in logs | Medium | Medium | Never log tokens. Log only chat IDs and URLs. |
| Pipeline panics crash bot task | Low | Medium | `process_url()` already catches all errors and returns `IngestResult::Failed`. Bot tasks catch panics at the spawn boundary. |
| Binary size increase from new deps | Low | Low | Both crates share existing deps (tokio, reqwest, serde). Incremental increase is small. |
| Missing bot token env var prevents HTTP server from starting | Medium | Medium | Fail-fast is intentional — a misconfigured bot section should be fixed, not silently ignored. The error message clearly states which env var is missing. |
| Concurrent pipeline calls race on filesystem writes | Low | Low | Existing issue shared with the HTTP endpoint. Two notes with identical titles could overwrite. Acceptable for personal use; a future SQLite queue would serialize writes. |

## Open Questions

- [x] ~~Should bot tokens be in the config file or env vars?~~ Env vars, via `bot_token_env` indirection (same pattern as `groq.api_key_env`).
- [x] ~~Should bots support tags?~~ Not in v1. Tags can be added later by parsing hashtags from the message text alongside the URL.
- [ ] Should the bot echo back the full note content or just a summary? Starting with title + status + path.

## References

- [Telegram Bot API](https://core.telegram.org/bots/api) — official API documentation
- [teloxide](https://github.com/teloxide/teloxide) — Rust Telegram bot framework (v0.17)
- [serenity](https://github.com/serenity-rs/serenity) — Rust Discord library (v0.12)
- [obsidian-ingestion-ideas.md](../obsidian-ingestion-ideas.md) — original ideas document, Paradigm 1
- [simplify-single-crate design doc](2026-03-06-simplify-single-crate.md) — current codebase architecture
