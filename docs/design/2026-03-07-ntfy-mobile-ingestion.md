# Design Document: ntfy.sh Mobile Ingestion Source

**Author:** Scott Idler
**Date:** 2026-03-07
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add ntfy.sh as a new ingestion source so URLs can be shared from an Android phone (Google Discover, YouTube app) to obsidian-borg in two taps — Share → "Borg". The daemon subscribes to an ntfy.sh topic via a streaming JSON connection, and the phone publishes URLs to the same topic using the HTTP Shortcuts Android app. No VPN, port forwarding, or Tailscale required.

## Problem Statement

### Background

obsidian-borg currently supports five ingestion sources: Telegram bot, Discord bot, HTTP POST, clipboard hotkey, and CLI. On desktop, ingestion is fast — clipboard hotkey is one keystroke. But on mobile (Pixel phone), the best available path is:

1. Tap Share in YouTube/Chrome
2. Scroll to find Telegram
3. Tap Telegram
4. Find the obsidian-borg bot chat
5. Tap Send

That's five interactions with three app context switches. The primary mobile discovery surfaces — Google Discover and the YouTube app — generate the majority of URLs worth ingesting, so this friction directly impacts capture rate.

### Problem

There is no low-friction mobile ingestion path. The Telegram relay works but requires navigating through Telegram's UI to find the right chat. The HTTP endpoint requires the daemon to be network-reachable from the phone, which means either Tailscale, port forwarding, or a reverse proxy — infrastructure that may not always be available (e.g., on cellular, guest WiFi).

### Goals

- Two-tap ingestion from any Android share sheet: Share → tap "Borg" → done
- No VPN or port forwarding required — works from any network
- New `Ntfy` variant in `IngestMethod` enum for ledger tracking
- Config-driven: ntfy section in `obsidian-borg.yml`, skipped if absent
- Follow the existing bot spawn pattern (like Telegram/Discord in `run_server()`)
- Optional authentication via ntfy access tokens

### Non-Goals

- Building a custom Android app — HTTP Shortcuts handles the share-target UX
- Push notifications back to the phone on ingest completion (future enhancement)
- Supporting ntfy.sh as a publish target (we only subscribe)
- Self-hosting an ntfy server — the public ntfy.sh instance is sufficient
- iOS support (Pixel-only for now; HTTP Shortcuts exists on iOS but untested)

## Proposed Solution

### Overview

```
Phone                          ntfy.sh                    obsidian-borg daemon
─────                          ───────                    ────────────────────
Share → HTTP Shortcuts    ──>  POST /topic         ──>    JSON subscribe /topic/json
       (2 taps)                (public relay)              extract URL
                                                           pipeline::process_url()
                                                           write note to vault
```

The daemon opens a long-lived JSON streaming connection to `https://ntfy.sh/<topic>/json` on startup. When a message arrives, it extracts the URL from the message body and feeds it through the standard pipeline with `IngestMethod::Ntfy`.

### Architecture

#### ntfy.sh Protocol

ntfy.sh is a simple pub/sub service. Publishing is an HTTP PUT/POST with the message as the body. Subscribing is an SSE stream or JSON stream. We use the JSON stream (`/json`) because it's simpler to parse than SSE and carries the same data.

**Publish (phone side):**
```
POST https://ntfy.sh/<topic>
Authorization: Bearer <token>   # optional
Content-Type: text/plain

https://youtube.com/watch?v=abc123
```

**Subscribe (daemon side):**
```
GET https://ntfy.sh/<topic>/json
Authorization: Bearer <token>   # optional
```

This opens a long-lived HTTP connection. ntfy sends one JSON object per line for each new message. On first connect we omit `since=` so we only see new messages (the daemon relies on being always-on; if it restarts, any messages published during downtime within ntfy's 12-hour cache can be recovered with `since=<duration>`, but dedup via Borg Log makes this non-critical). On reconnect after a drop, we use `since=<last-event-id>` to resume without gaps.

**ntfy JSON event format:**
```json
{
  "id": "sPs7MCIL0fU",
  "time": 1709839200,
  "expires": 1709882400,
  "event": "message",
  "topic": "obsidian-borg-a7f3b9c2",
  "message": "https://youtube.com/watch?v=abc123"
}
```

Events with `"event": "keepalive"` or `"event": "open"` must be ignored — only process `"event": "message"`.

#### Message Format

The simplest approach: the message body IS the URL. One URL per message. This matches how Android's share sheet works — it shares a single URL as text.

**Important:** Android's share sheet sometimes sends more than just the URL. Google Discover may share `"Article Title\nhttps://example.com/..."` and YouTube may share `"Check out this video: https://youtube.com/..."`. The message parser must extract the first URL from the message text using the same `extract_url_from_text()` function already used by the Telegram and Discord bots in `router.rs`.

For future extensibility, we also support JSON message bodies:
```json
{"url": "https://...", "tags": ["ai", "rust"], "force": true}
```

Detection is simple: if the trimmed message starts with `{`, parse as JSON `IngestRequest`; otherwise extract the URL from the plain text.

#### New Module: `src/ntfy.rs`

```rust
pub async fn run(
    server: String,       // e.g., "https://ntfy.sh"
    topic: String,        // e.g., "obsidian-borg-a7f3b9c2"
    token: Option<String>,// resolved secret (not path)
    config: Arc<Config>,
) -> Result<()> {
    let mut last_event_id: Option<String> = None;
    let mut backoff = ExponentialBackoff::new();

    loop {
        // 1. Build URL: {server}/{topic}/json[?since={last_event_id}]
        // 2. Open reqwest streaming GET with optional Bearer token
        // 3. Read lines from response body (one JSON object per line)
        // 4. For each line:
        //    a. Parse as ntfy JSON event
        //    b. Skip if event != "message"
        //    c. Store event id in last_event_id
        //    d. Extract URL from message field via router::extract_url_from_text()
        //    e. Spawn tokio task: pipeline::process_url(url, tags, IngestMethod::Ntfy, ...)
        // 5. On stream end/error: log warning, backoff.wait(), reconnect with since=last_event_id
        // 6. On successful message: backoff.reset()
    }
}
```

The function signature matches how it's called from `run_server()` — secrets are resolved at the call site (consistent with Telegram/Discord pattern).

#### Reconnection Strategy

ntfy.sh connections can drop (server restart, network blip). The subscriber must reconnect with exponential backoff:

| Attempt | Delay |
|---------|-------|
| 1 | 1s |
| 2 | 2s |
| 3 | 4s |
| 4 | 8s |
| 5+ | 30s (cap) |

On successful message receipt, reset the backoff counter. Use `since=<last-event-id>` to avoid reprocessing messages (ntfy includes an `id` field in each JSON event). The last event ID is stored in memory only — if the daemon fully restarts, it connects without `since=` and only sees new messages. This is acceptable because:

1. The daemon is a systemd service with `Restart=always` — downtime is brief
2. ntfy caches messages for 12 hours, so a `since=10m` fallback could recover short outages
3. Dedup via Borg Log prevents double-processing if messages overlap on reconnect
4. Worst case (daemon down for hours): URLs shared during downtime are lost, which is the same as not sharing them at all — user can re-share

#### Config Changes

Add to `obsidian-borg.yml`:

```yaml
# Optional: ntfy.sh (mobile share target)
ntfy:
  topic: "obsidian-borg-<random>"   # unique topic name (acts as a shared secret)
  server: "https://ntfy.sh"         # optional, default: https://ntfy.sh
  token: "~/.config/ntfy/token"     # optional, file path or env var
```

Add to `config.rs`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct NtfyConfig {
    pub topic: String,
    #[serde(default = "default_ntfy_server")]
    pub server: String,
    pub token: Option<String>,
}

fn default_ntfy_server() -> String {
    "https://ntfy.sh".to_string()
}
```

#### IngestMethod Update

Add `Ntfy` to the enum in `types.rs`:

```rust
pub enum IngestMethod {
    Telegram,
    Discord,
    Http,
    Clipboard,
    Cli,
    Ntfy,  // new
}
```

Display as `"ntfy"` in frontmatter and ledger.

#### Daemon Wiring

In `lib.rs::run_server()`, add after the Discord block:

```rust
// ntfy subscriber (config-driven)
if let Some(ntfy_config) = &config.ntfy {
    let server = ntfy_config.server.clone();
    let topic = ntfy_config.topic.clone();
    let token = ntfy_config.token.as_ref()
        .and_then(|t| config::resolve_secret(t).ok());
    let cfg = config.clone();
    tasks.spawn(async move { ntfy::run(server, topic, token, cfg).await });
    println!("{} ntfy subscriber active (topic: {})", "-->".green(), ntfy_config.topic);
}
```

### Phone-Side Setup

#### HTTP Shortcuts App Configuration

1. Install [HTTP Shortcuts](https://play.google.com/store/apps/details?id=ch.rmy.android.http_shortcuts) from Play Store
2. Create a new shortcut:
   - **Name:** Borg
   - **Method:** POST
   - **URL:** `https://ntfy.sh/<topic>`
   - **Headers:** `Authorization: Bearer <token>` (if using auth)
   - **Body:** `{share_text}` (HTTP Shortcuts variable for shared content)
3. Enable "Share into shortcut" in shortcut settings
4. Optionally set an icon (the borg cube emoji or similar)

After setup, "Borg" appears in the Android share sheet. Tap it and the URL is published to ntfy.sh instantly.

### Implementation Plan

**Phase 1: Core ntfy subscriber**
- Add `NtfyConfig` to `config.rs`
- Add `Ntfy` variant to `IngestMethod`
- Implement `src/ntfy.rs` with JSON stream subscription, URL extraction, pipeline integration
- Wire into `run_server()` task set
- Update `obsidian-borg.example.yml`

**Phase 2: Robustness**
- Exponential backoff reconnection
- `since=<last-event-id>` tracking for seamless reconnects
- JSON body support (tags, force flag)
- Unit tests for message parsing

**Phase 3: Documentation**
- README section for ntfy setup
- Phone-side HTTP Shortcuts import file (shareable config)

## Alternatives Considered

### Alternative 1: Tailscale + Direct HTTP POST
- **Description:** Use Tailscale VPN to make the daemon's HTTP endpoint reachable from the phone, then use HTTP Shortcuts to POST directly to `/ingest`
- **Pros:** No relay service, lower latency, simpler server code (no new module)
- **Cons:** Requires Tailscale on the phone and on the borg host; breaks on networks that block VPN; adds infrastructure dependency
- **Why not chosen:** ntfy.sh works from any network with zero infrastructure. Tailscale is a fine option for users who already have it, but shouldn't be a requirement.

### Alternative 2: Telegram (status quo)
- **Description:** Continue using Telegram as the mobile relay
- **Pros:** Already implemented, no new code
- **Cons:** Five interactions to send a URL; requires finding the bot chat every time; context-switches through Telegram's UI
- **Why not chosen:** Too much friction. The whole point is reducing taps.

### Alternative 3: Custom Android App
- **Description:** Build a minimal Android app that appears in the share sheet and POSTs to borg
- **Pros:** Maximum control over UX, could show ingest status
- **Cons:** Requires building and maintaining an Android app; Play Store publishing; still needs a relay or VPN for connectivity
- **Why not chosen:** HTTP Shortcuts already provides the share-target UX. Building an app is disproportionate effort for what is a one-POST operation.

### Alternative 4: Pushbullet / Pushover
- **Description:** Use Pushbullet or Pushover as the relay instead of ntfy.sh
- **Pros:** Established services with good mobile apps
- **Cons:** Paid services (Pushbullet Pro for API, Pushover one-time fee); proprietary; less control over message format; ntfy.sh is open source, free, and self-hostable
- **Why not chosen:** ntfy.sh is simpler, free, open source, and its HTTP API is trivial to integrate.

### Alternative 5: MQTT Broker
- **Description:** Use an MQTT broker (Mosquitto, HiveMQ Cloud) as the pub/sub relay
- **Pros:** Robust pub/sub protocol, exactly-once delivery options
- **Cons:** Heavier protocol; requires MQTT client on phone (no native share-sheet integration without extra app); overkill for single-URL messages
- **Why not chosen:** ntfy.sh is purpose-built for this use case and has the simplest possible publish interface (HTTP POST).

## Technical Considerations

### Dependencies

- **reqwest** (already in Cargo.toml) — used for streaming JSON subscription via `response.bytes_stream()`
- **serde_json** (already in Cargo.toml) — parse ntfy JSON events (one per line)
- **No new crate dependencies required** — ntfy's JSON stream format is one JSON object per line, trivially parsed with `serde_json::from_str()` on each line from the response stream. No SSE library needed.

### Performance

- One long-lived HTTP connection per daemon instance
- Messages arrive within ~100ms of publish (ntfy.sh latency)
- Pipeline processing is the bottleneck (Fabric calls, LLM summarization), not the relay
- Memory: negligible — one HTTP connection, messages processed and dropped

### Security

- **Topic name as shared secret:** The ntfy topic name should be random/unguessable (e.g., `obsidian-borg-a7f3b9c2`). Anyone who knows the topic can publish to it.
- **Optional access tokens:** ntfy.sh supports token-based auth. If configured, both the phone and daemon use the same token. Token resolved via `config::resolve_secret()` (file or env var).
- **No sensitive data in transit:** Messages contain only URLs. The URL itself is not secret — it's a public YouTube/article link.
- **Self-hosting option:** For maximum control, users can self-host ntfy (`docker run binwiederhier/ntfy`). The `server` config field supports any ntfy-compatible endpoint.

### Testing Strategy

- **Unit tests:** Message parsing (plain URL vs JSON body, URL extraction from "Check out: https://..." text), config deserialization, ntfy JSON event parsing (message vs keepalive vs open)
- **Integration test:** Mock HTTP server that sends newline-delimited JSON events, verify pipeline receives correct URLs with `IngestMethod::Ntfy`
- **Manual test:** Publish to ntfy topic via curl, verify note appears in vault

### Rollout Plan

1. Implement and merge
2. Add `ntfy:` section to Scott's config with a random topic
3. Set up HTTP Shortcuts on Pixel
4. Test with a few YouTube shares
5. If stable, document in README

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| ntfy.sh downtime | Low | Med | ntfy.sh has strong uptime; self-hosting is fallback; messages cached 12h server-side |
| Topic discovery by third party | Low | Low | Random topic name + optional token auth; worst case is spam URLs that fail pipeline |
| JSON stream drops silently | Med | Low | Reconnect with backoff; `since=<id>` ensures no missed messages; dedup handles overlap |
| HTTP Shortcuts app discontinued | Low | Med | Any HTTP client app works (Tasker, MacroDroid); could also use ntfy's own Android app as share target |
| Message format ambiguity | Low | Low | Simple heuristic: starts with `{` = JSON, otherwise plain URL; validate URL before processing |
| Shared text contains no URL | Low | Low | Android sometimes shares title-only text; `extract_url_from_text()` returns None; log and skip |
| Duplicate messages on reconnect | Med | None | Borg Log dedup catches these; `since=<id>` minimizes overlap |

## Open Questions

- [ ] Should we support batch URLs in a single ntfy message (one per line)? Leaning no — Android share sheet sends one URL at a time.
- [ ] Should the daemon publish ingest results back to a separate ntfy topic for phone notifications? Useful but out of scope for v1.

## References

- [ntfy.sh documentation](https://docs.ntfy.sh/)
- [ntfy.sh JSON subscription](https://docs.ntfy.sh/subscribe/api/#subscribe-as-json-stream)
- [ntfy.sh authentication](https://docs.ntfy.sh/publish/#authentication)
- [HTTP Shortcuts Android app](https://http-shortcuts.rmy.ch/)
- Existing design doc: `docs/design/2026-03-07-canonicalization-dedup-dashboard.md`
