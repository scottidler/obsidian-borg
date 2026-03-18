# Design Document: Unified Telegram Notifications

**Author:** Scott Idler
**Date:** 2026-03-18
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Extract notification logic from `telegram.rs` into a standalone `Notifier` service that sends "Processing..." and "Saved: ..." feedback via Telegram for ALL ingestion methods (HTTP, CLI, ntfy, Discord, clipboard), not just Telegram-originated ingests. The Telegram handler becomes a pure input parser. The notifier accepts an optional `override_chat_id` so Telegram-originated messages reply to the sender, while other methods use a configured default destination. If the Telegram bot is unavailable, notifications fail gracefully with a log warning.

## Problem Statement

### Background

obsidian-borg currently provides rich feedback when content is ingested via Telegram: an immediate `[tg-abc123] Processing...` receipt followed by a detailed `Saved: Title (2.5s)\nTags: ...\nDomain: ...` summary with an "Open in Obsidian" deep link. This two-stage feedback is valuable - it confirms receipt instantly and provides a searchable record of what was ingested.

Other ingestion methods have no comparable feedback:
- **HTTP/CLI:** Returns JSON to the caller or prints to stdout. No push notification.
- **ntfy:** Logs only. No user-facing feedback at all.
- **Discord:** Has its own notification path (duplicated logic from Telegram).
- **Clipboard hotkey:** Desktop notification via `notify-rust`, no Telegram feedback.

### Problem

1. **Inconsistent user experience.** Ingesting via ntfy or the browser extension gives no confirmation that the note was created, what it was titled, or what domain it was routed to. Scott has to check the vault manually.

2. **Duplicated notification code.** `telegram.rs` and `discord.rs` each contain their own notification formatting and sending logic. The `format_telegram_reply()` function is private to `telegram.rs` but does the same job as `format_discord_reply()` in `discord.rs`, both wrapping the shared `format_reply()` from `router.rs`.

3. **No single feedback channel.** Scott already monitors the Telegram bot chat. Having all ingestion feedback flow there - regardless of origin method - creates a unified activity log that is always visible on his phone.

### Goals

- All ingestion methods send "Processing..." and result notifications via Telegram
- Telegram handler delegates notification responsibility to the shared notifier
- Notifier supports an optional `override_chat_id` for reply-to-sender behavior
- Graceful degradation: if Telegram is unavailable, log a warning and continue silently
- Reduce code duplication between `telegram.rs` and `discord.rs` notification paths

### Non-Goals

- Replacing Discord's own reply mechanism (Discord users still get replies in Discord)
- Adding notification channels beyond Telegram (Slack, email, etc.)
- Changing the notification message format (reuse existing `format_reply()`)
- Modifying the pipeline itself - notifications remain the caller's responsibility
- Mobile push notifications via ntfy (ntfy is an input source, not an output)

## Proposed Solution

### Overview

Create a `src/notify.rs` module containing a `TelegramNotifier` struct. It holds a `teloxide::Bot` instance and a default `chat_id`. All ingestion methods call the notifier instead of implementing their own send logic. The Telegram handler passes `Some(message.chat.id)` as an override; other methods pass `None` to use the configured default.

### Architecture

```
                    Ingestion Methods
                    =================
    Telegram    HTTP/CLI    ntfy    Discord    Clipboard
        |          |         |         |          |
        |          |         |         |          |
        v          v         v         v          v
    (parse)    (parse)   (parse)   (parse)    (parse)
        |          |         |         |          |
        +----------+---------+---------+----------+
                           |
                    notify.processing()
                    (override_chat_id)
                           |
                    pipeline::process_content()
                           |
                    notify.result()
                    (override_chat_id)
```

### Data Model

**New config field in `TelegramConfig`:**

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TelegramConfig {
    #[serde(alias = "bot_token_env", alias = "bot_token")]
    pub bot_token: String,
    #[serde(default, alias = "allowed_chat_ids")]
    pub allowed_chat_ids: Vec<i64>,
    /// Default chat ID for cross-method notifications.
    /// If not set, falls back to first allowed_chat_ids entry.
    #[serde(default, alias = "notification_chat_id")]
    pub notification_chat_id: Option<i64>,
    #[serde(default)]
    pub host: Option<String>,
}
```

**Config YAML:**

```yaml
telegram:
  bot-token: TELEGRAM_BOT_TOKEN
  allowed-chat-ids: [123456789]
  notification-chat-id: 123456789
```

### API Design

**`src/notify.rs` - Public Interface:**

```rust
/// Shared notification service. Sends feedback via Telegram.
/// Clone-cheap (inner Arc).
#[derive(Clone)]
pub struct Notifier {
    bot: teloxide::Bot,
    default_chat_id: teloxide::types::ChatId,
}

impl Notifier {
    /// Build from a resolved bot token and config.
    /// Chat ID resolution order:
    ///   1. tg_config.notification_chat_id (explicit)
    ///   2. tg_config.allowed_chat_ids[0] (implicit)
    ///   3. None -> returns None (no destination, notifier disabled)
    /// At startup, logs which chat ID is being used.
    pub fn new(token: &str, tg_config: &TelegramConfig) -> Option<Self>;

    /// Send "[trace_id] Processing..." message.
    /// Uses override_chat_id if provided, else default.
    /// Returns Ok on success so callers can await delivery before
    /// starting the pipeline (important for ordering guarantees).
    /// On failure, logs a warning and returns Err (callers should
    /// ignore the error and proceed).
    pub async fn processing(
        &self,
        trace_id: &str,
        description: &str,
        override_chat_id: Option<i64>,
    ) -> Result<(), ()>;

    /// Send the full result message (Saved/Duplicate/Failed).
    /// Uses override_chat_id if provided, else default.
    /// Fire-and-forget: logs on failure, never errors the caller.
    pub async fn result(
        &self,
        result: &IngestResult,
        display_source: &str,
        override_chat_id: Option<i64>,
    );
}

/// Format an IngestResult as HTML for Telegram.
/// Moved from telegram.rs - now shared.
pub fn format_telegram_reply(result: &IngestResult, display_source: &str) -> String;

/// Escape HTML special characters for Telegram messages.
pub fn html_escape(s: &str) -> String;
```

`processing()` returns `Result<(), ()>` so callers can optionally await it for ordering (Telegram handler awaits to ensure "Processing..." arrives before the result). `result()` returns `()` and is always fire-and-forget. Both log warnings internally on failure.

**Note on Bot instances:** The Telegram handler's `teloxide::Dispatcher` owns its own `Bot` for polling (`get_updates`). The `Notifier` creates a separate `Bot` instance from the same token. This is safe - `Bot` is a stateless HTTP client wrapper. Only `send_message` is called, never `get_updates`, so there is no conflict with the polling dispatcher.

**Shared state change:**

The daemon currently passes `Arc<Config>` as axum state. This changes to a struct that includes the optional notifier:

```rust
pub struct AppState {
    pub config: Arc<Config>,
    pub notifier: Option<Notifier>,
}
```

### Implementation Plan

**Phase 1: Extract notifier (`src/notify.rs`)**
- Create `Notifier` struct with `new()`, `processing()`, `result()`
- Move `html_escape()` and `format_telegram_reply()` from `telegram.rs` to `notify.rs`
- `telegram.rs` imports from `notify` instead of having its own copies
- Tests: unit tests for `format_telegram_reply`, `html_escape`, `Notifier::new` returns None when no chat_id available

**Phase 2: Wire into daemon startup and refactor Telegram handler**
- In `run_server()`: after resolving the Telegram bot token, build `Option<Notifier>` using `Notifier::new(token, tg_config)`
- Create `AppState { config, notifier }` and use `Arc<AppState>` as axum state
- Pass `Notifier` clone to `ntfy::run()` and `telegram::run()`
- Refactor `telegram.rs`: replace all `bot.send_message()` calls for "Processing..." and result notifications with `notifier.processing()` / `notifier.result()`, passing `Some(chat_id)` as the override
- The handler still uses its own `Bot` instance for file downloads and the polling dispatcher

After this phase, the Telegram handler endpoint looks like:

```rust
// Simplified - URL/text case
let trace_id = trace::generate(IngestMethod::Telegram);
let _ = notifier.processing(&trace_id, "Processing...", Some(chat_id.0)).await;

let notifier_clone = notifier.clone();
tokio::spawn(async move {
    let result = pipeline::process_content(content, tags, method, force, &config, Some(trace_id)).await;
    notifier_clone.result(&result, &display_source, Some(chat_id.0)).await;
});
```

**Phase 3: Other methods use notifier**
- `routes.rs`: Extract `notifier` from `AppState`. Call `notifier.processing()` before and `notifier.result()` after `process_content()`. Pass `None` for override_chat_id (uses default).
- `ntfy.rs`: Same pattern inside the `tokio::spawn` blocks.
- CLI path: `run_ingest()` posts to the HTTP endpoint, which is handled by `routes.rs` - so it gets notifications automatically.
- `discord.rs`: Continues sending its own Discord replies. Additionally fires the Telegram notifier for cross-channel visibility (open question - see below).

**Phase 4: Config and docs**
- Add `notification-chat-id` to `obsidian-borg.example.yml`
- Update CLAUDE.md architecture diagram to show Notifier

## Alternatives Considered

### Alternative 1: Notification inside the pipeline
- **Description:** Move notification calls into `pipeline::process_content()` itself, so every caller gets notifications automatically.
- **Pros:** Zero changes needed in callers. Guaranteed consistency.
- **Cons:** Pipeline becomes coupled to Telegram. Harder to test. The pipeline is a pure processing function today - adding side effects changes its character. Would need to thread `Notifier` through every pipeline call.
- **Why not chosen:** Violates separation of concerns. Callers should decide whether/how to notify.

### Alternative 2: Event bus / channel
- **Description:** Pipeline emits events (`Processing`, `Completed`, `Failed`) on a `tokio::broadcast` channel. A listener in `notify.rs` consumes them and sends Telegram messages.
- **Pros:** Fully decoupled. Could add more listeners later (Discord, Slack, webhooks).
- **Cons:** Over-engineered for current needs. Adds indirection. Harder to pass `override_chat_id` context through a generic event bus.
- **Why not chosen:** YAGNI. One notification destination (Telegram) doesn't justify the complexity.

### Alternative 3: Keep Telegram handler's own notifications, add notifier for others only
- **Description:** Original proposal before Scott's feedback. Telegram handler keeps its send logic; notifier only serves non-Telegram methods.
- **Pros:** Smaller change to `telegram.rs`.
- **Cons:** Two code paths doing the same thing. Telegram handler's notification logic can drift from the notifier's. More code to maintain.
- **Why not chosen:** Scott preferred unifying to a single notification path.

## Technical Considerations

### Dependencies

- `teloxide` - already a dependency. The `Notifier` uses `teloxide::Bot` directly for `send_message`.
- No new crates required.

### Performance

- Telegram API calls are async and non-blocking.
- `processing()` is awaited by the Telegram handler (preserves current ordering: receipt appears before result). Other callers (ntfy, HTTP) can choose to await or spawn.
- `result()` is fire-and-forget in all cases (spawned inside the async pipeline task).
- Telegram rate limits: 30 messages/second to the same chat. Not a concern at personal-use volumes.

### Security

- Bot token is already handled via `resolve_secret()`. No change.
- `notification-chat-id` is not sensitive (it's a chat identifier, not a credential).
- HTML escaping via `html_escape()` prevents injection in Telegram messages.

### Testing Strategy

- **Unit tests:** `format_telegram_reply()`, `html_escape()`, `Notifier::new()` returns `None` when no chat_id available.
- **Integration-style:** `Notifier` methods log warnings on send failure - verify with a mock or unreachable bot token.
- **Manual:** Ingest via ntfy/CLI/HTTP and verify Telegram notification appears.

### Rollout Plan

1. Implement phases 1-2. At this point, Telegram-originated ingests work identically but through the new notifier code path.
2. Deploy and verify no regression in Telegram behavior.
3. Implement phase 3. Non-Telegram methods now send notifications.
4. Add `notification-chat-id` to production config and deploy.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Telegram API down causes log noise | Low | Low | Rate-limit warning logs (log once per backoff cycle, not per message) |
| Bot token shared between handler and notifier causes polling conflicts | Low | Medium | Notifier only calls `send_message`, never `get_updates`. No conflict with polling. |
| Double notifications during migration (old + new code path) | Medium | Low | Phase 2 removes old Telegram send logic and replaces it with notifier calls in the same commit. No window where both paths are active. |
| `notification-chat-id` not configured, no notifications | Medium | Low | Fall back to first `allowed_chat_ids` entry. Log info at startup about notification destination. |

## Edge Cases

### Telegram download errors
The Telegram handler currently sends error messages directly via `bot.send_message()` when file downloads fail (e.g. "Failed to download photo: ..."). These are error-handling messages, not ingest feedback. They stay as direct `bot.send_message()` calls since they happen before the pipeline runs and are specific to the Telegram input path.

### Direct CLI ingestion (no daemon)
`run_note()` and `run_file_ingest()` call `process_content()` directly without going through the HTTP endpoint. These paths won't get Telegram notifications unless we build a `Notifier` in those functions too. This is acceptable for now - the primary use case (`obsidian-borg ingest <url>`) goes through the daemon's HTTP endpoint. Direct file/note ingestion is a secondary path that can be wired in later if needed.

### Token validation at startup
`Notifier::new()` should verify the token works (e.g. a `bot.get_me()` call) to fail fast at startup rather than silently failing on every notification. If the check fails, log a warning and return `None` (notifier disabled).

## Open Questions

- [ ] Should Discord ingests also trigger Telegram notifications for cross-channel visibility, or is that noisy?
- [ ] Should the "Processing..." message be editable (Telegram supports `edit_message_text`) so the final result replaces it instead of being a separate message? This would reduce chat clutter but adds complexity.
- [ ] Should direct CLI commands (`obsidian-borg note`, `obsidian-borg ingest --file`) also send Telegram notifications? Would require building a Notifier outside the daemon context.

## References

- Existing notification code: `src/telegram.rs:99-107` (`format_telegram_reply`), `src/router.rs:83-122` (`format_reply`)
- Trace ID design: `docs/design/2026-03-16-trace-id.md`
- Teloxide `send_message` API: teloxide crate docs
