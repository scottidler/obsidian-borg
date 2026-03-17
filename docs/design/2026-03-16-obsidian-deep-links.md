# Design Document: Obsidian Deep Links in Bot Replies

**Author:** Scott Idler
**Date:** 2026-03-16
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Add clickable `obsidian://open` deep links to Telegram (and Discord) bot replies so that after ingestion, the user can tap to open the newly created note directly in Obsidian on their phone. This requires a new `vault_name` config field, a URI builder, HTML parse mode for Telegram, and minor changes to the reply formatter.

## Problem Statement

### Background

obsidian-borg's Telegram bot currently replies with a plain-text summary after ingesting content:

```
[tg-7f3a2c] Saved: Claude Code Obsidian Guide (4.6s)
Tags: #ai, #llm, #obsidian
Folder: 📥 Inbox
```

The user sees the note was created but has no way to navigate to it from the Telegram chat. They must manually open Obsidian, navigate to the folder, and find the note by title.

### Problem

There is no clickable link from the bot reply to the actual note in Obsidian. On mobile (the primary Telegram use case), this means extra steps every time content is ingested. Obsidian supports a custom URI scheme (`obsidian://open?vault=...&file=...`) that can deep-link directly to a note from any app, including Telegram.

### Goals

- Generate `obsidian://open` URIs for successfully ingested notes
- Include a clickable "Open in Obsidian" link in Telegram bot replies
- Include the link in Discord bot replies (plain text - Discord doesn't support custom URI schemes in embeds)
- Add `vault_name` to config so the URI can target the correct vault
- Include the `obsidian_url` field in HTTP API JSON responses for programmatic consumers

### Non-Goals

- Supporting Obsidian Advanced URI plugin (extra plugin, not needed for basic open)
- Deep-linking to a specific heading or block within a note
- Handling multiple vaults (single vault per obsidian-borg instance)
- Making the link work on platforms without Obsidian installed (graceful degradation is fine)

## Proposed Solution

### Overview

1. Add `vault_name: String` to `VaultConfig` in config.
2. Add `obsidian_url: Option<String>` to `IngestResult` - computed in the pipeline after note is written.
3. Build the `obsidian://open?vault={name}&file={rel_path}` URI by stripping `vault.root_path` from the absolute `note_path` and percent-encoding the components.
4. Add `format_telegram_reply()` in `telegram.rs` that wraps the base reply in HTML with a clickable deep link. `format_reply()` in `router.rs` is unchanged.
5. Switch Telegram reply `send_message` calls to HTML parse mode.
6. Discord gets a plain-text URI (user can copy/paste on desktop, or tap on mobile).

### Before / After (Telegram)

**Before** (plain text, no link):
```
[tg-7f3a2c] Saved: Claude Code Obsidian Guide (4.6s)
Tags: #ai, #llm, #obsidian
Folder: 📥 Inbox
```

**After** (HTML, clickable link):
```
[tg-7f3a2c] Saved: Claude Code Obsidian Guide (4.6s)
Tags: #ai, #llm, #obsidian
Folder: 📥 Inbox
Open in Obsidian          <-- tappable link
```

The "Open in Obsidian" text is an HTML `<a>` tag pointing to `obsidian://open?vault=obsidian&file=...`. Tapping it on mobile opens the note directly in the Obsidian app. Telegram shows a confirmation popup before opening the custom URI scheme.

### Architecture

```
Pipeline writes note
    |
    v
note_path = "/home/user/obsidian/📥 Inbox/claude-code-guide.md"
vault_root = "/home/user/obsidian/"
vault_name = "obsidian"
    |
    v
rel_path = "📥 Inbox/claude-code-guide.md"
    |
    v
obsidian_url = "obsidian://open?vault=obsidian&file=%F0%9F%93%A5%20Inbox%2Fclaude-code-guide.md"
    |
    v
IngestResult.obsidian_url = Some(obsidian_url)
    |
    +-- Telegram: HTML <a href="obsidian://...">Open in Obsidian</a>
    +-- Discord:  plain text obsidian://... (clickable on mobile)
    +-- HTTP API: JSON field "obsidian_url": "obsidian://..."
```

### Data Model

#### Config change: `VaultConfig`

```rust
pub struct VaultConfig {
    pub root_path: String,
    pub inbox_path: String,
    pub vault_name: String,    // NEW: Obsidian vault name for deep links
}
```

Default: `"obsidian"`. This matches the directory name convention (`~/repos/scottidler/obsidian/`). Users whose vault has a different name (e.g., "My Notes") must set this explicitly in config. A future enhancement could auto-derive it from the last path component of `root_path`, but the simple default covers the common case.

Config YAML:
```yaml
vault:
  root_path: ~/repos/scottidler/obsidian/
  inbox_path: ~/repos/scottidler/obsidian/📥 Inbox
  vault_name: obsidian
```

#### IngestResult change

```rust
pub struct IngestResult {
    // ... existing fields ...
    pub obsidian_url: Option<String>,  // NEW
}
```

Set on `Completed` status, `None` otherwise. Serialized in HTTP JSON responses automatically via serde.

### API Design

#### New helper: `build_obsidian_url()`

Located in `pipeline.rs` alongside the existing `expand_tilde()` (which is needed to resolve the vault root path). This avoids importing or duplicating `expand_tilde` in another module.

```rust
/// Build an obsidian://open deep link from vault name and note path.
///
/// `note_path` is the absolute filesystem path to the written note.
/// `vault_root` is the unexpanded config value (e.g., "~/repos/scottidler/obsidian/").
/// Returns None if note_path doesn't start with the expanded vault_root.
fn build_obsidian_url(vault_name: &str, note_path: &str, vault_root: &str) -> Option<String> {
    let expanded_root = expand_tilde(vault_root);
    let root_str = expanded_root.to_string_lossy();
    // Ensure root ends with / for clean stripping
    let root_prefix = if root_str.ends_with('/') {
        root_str.to_string()
    } else {
        format!("{root_str}/")
    };

    let rel_path = note_path.strip_prefix(&root_prefix)?;

    let encoded_vault = urlencoding::encode(vault_name);
    let encoded_file = urlencoding::encode(rel_path);

    Some(format!("obsidian://open?vault={encoded_vault}&file={encoded_file}"))
}
```

Called at each `IngestResult` construction site in `pipeline.rs` (there are ~8 of them), right after `note_path` is computed:

```rust
let obsidian_url = build_obsidian_url(
    &config.vault.vault_name,
    &note_path.to_string_lossy(),
    &config.vault.root_path,
);
```

#### Updated `format_reply()` (router.rs)

`format_reply()` stays plain-text and does NOT include the obsidian URL. This keeps it simple and avoids mixing formatting concerns. The obsidian link is handled differently per consumer:

- **Telegram:** wraps it in HTML
- **Discord:** appends plain-text URI
- **HTTP API:** returns it as a JSON field (already via serde)

```rust
// format_reply() is UNCHANGED - it remains plain text
pub fn format_reply(result: &IngestResult, url: &str) -> String {
    // ... exactly as today ...
}
```

#### Telegram HTML formatting

In `telegram.rs`, a new `format_telegram_reply()` function builds an HTML message:

```rust
/// Format an IngestResult as an HTML Telegram message with an optional
/// clickable "Open in Obsidian" deep link.
fn format_telegram_reply(result: &IngestResult, display_source: &str) -> String {
    let base = format_reply(result, display_source);
    let escaped = html_escape(&base);

    match &result.obsidian_url {
        Some(url) => format!("{escaped}\n<a href=\"{url}\">Open in Obsidian</a>"),
        None => escaped,
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
}
```

All 5 reply `send_message` calls in the spawned tasks switch from:
```rust
let reply = format_reply(&result, &display_source);
if let Err(e) = bot_clone.send_message(chat_id, reply).await { ... }
```
to:
```rust
let reply = format_telegram_reply(&result, &display_source);
if let Err(e) = bot_clone.send_message(chat_id, reply)
    .parse_mode(teloxide::types::ParseMode::Html)
    .await { ... }
```

**Note:** The "Processing..." messages sent before spawning remain plain text (no parse mode needed) since they contain no user-generated content that needs escaping and no links.

### Implementation Plan

#### Phase 1: Config + URI builder + types

- Add `vault_name` to `VaultConfig` with default `"obsidian"`
- Add `urlencoding` crate to Cargo.toml
- Add `obsidian_url: Option<String>` to `IngestResult` (with `skip_serializing_if`)
- Implement `build_obsidian_url()` in `pipeline.rs` (where `expand_tilde` lives)
- Unit tests for URI construction (emoji paths, spaces, nested folders)
- Update example config with `vault_name`

#### Phase 2: Pipeline integration

- At each `IngestResult` construction site in `pipeline.rs` (~8 places), compute `obsidian_url`
- HTTP API gets the field for free via serde (no changes to `routes.rs`)

#### Phase 3: Telegram HTML replies

- Add `format_telegram_reply()` + `html_escape()` in `telegram.rs`
- Replace `format_reply()` calls with `format_telegram_reply()` in all 5 spawned reply tasks
- Add `.parse_mode(ParseMode::Html)` to those `send_message` calls
- "Processing..." messages remain plain text (no change needed)

#### Phase 4: Discord (optional)

- Append plain-text `obsidian://` URI to Discord replies (no parse mode change needed)
- Test on Discord mobile to confirm custom URI scheme is tappable

## Alternatives Considered

### Alternative 1: Compute obsidian URL in format_reply only

- **Description:** Pass vault config to `format_reply()` and compute the link there instead of storing it in `IngestResult`
- **Pros:** No struct change, fewer pipeline modifications
- **Cons:** HTTP API consumers don't get the link. `format_reply()` gains config dependency. Testing harder.
- **Why not chosen:** Putting it in `IngestResult` makes it available to all consumers (HTTP, Telegram, Discord, CLI) and keeps `format_reply()` a pure formatter.

### Alternative 2: Use Obsidian Advanced URI plugin

- **Description:** Use `obsidian://advanced-uri?vault=...&filepath=...` for richer linking (e.g., jump to heading)
- **Pros:** More features
- **Cons:** Requires the Advanced URI plugin to be installed. Core `obsidian://open` works out of the box.
- **Why not chosen:** Unnecessary dependency. Basic open is sufficient for our use case.

### Alternative 3: Return a wikilink instead of URI

- **Description:** Return `[[note-title]]` in the reply
- **Pros:** Obsidian-native format
- **Cons:** Not clickable in Telegram. Requires user to copy and paste into Obsidian search. Doesn't solve the problem.
- **Why not chosen:** Doesn't achieve the goal of one-tap navigation.

## Technical Considerations

### Dependencies

**New:**
- `urlencoding` crate (~5KB, stable, no transitive deps) - for percent-encoding vault name and file path

**Existing:**
- `teloxide` - already supports `.parse_mode(ParseMode::Html)` on `SendMessage`
- `serde` - `IngestResult` already derives `Serialize`/`Deserialize`

### Performance

Negligible. String formatting and percent-encoding are sub-microsecond operations. No API calls, no I/O.

### Security

- The `obsidian://` URI only opens a local file that already exists in the vault. No remote access.
- HTML escaping in Telegram replies prevents injection via note titles containing `<`, `>`, `&`.
- Vault name comes from config (not user input), so no injection risk there.

### Testing Strategy

- Unit test: `build_obsidian_url()` with simple path (pipeline.rs)
- Unit test: `build_obsidian_url()` with emoji folder name (`📥 Inbox`)
- Unit test: `build_obsidian_url()` with nested subfolder
- Unit test: `build_obsidian_url()` with trailing slash vs no trailing slash on root
- Unit test: `build_obsidian_url()` returns None when path doesn't match root
- Unit test: `html_escape()` escapes `<`, `>`, `&` (telegram.rs)
- Unit test: `format_reply()` is unchanged - existing tests remain green (router.rs)
- Integration: send test URL via Telegram, verify reply contains clickable "Open in Obsidian" link

### Rollout Plan

1. Implement and test locally with `cargo test`
2. Add `vault_name: obsidian` to production config
3. Deploy, ingest a test URL via Telegram
4. Verify the "Open in Obsidian" link appears and works on phone

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Vault name mismatch between config and phone | Medium | Medium | Document clearly in example config. Default to directory name. |
| Telegram shows confirmation popup for custom URIs | High | Low | Expected behavior per Telegram docs. User taps "Open" once. |
| HTML parse mode breaks existing reply text | Low | Medium | `html_escape()` handles `<`, `>`, `&` in titles and tags. |
| Obsidian not installed on device | Low | Low | Link just won't open. Plain text reply still shows title/folder. |
| Emoji in path causes encoding issues | Low | Medium | `urlencoding` handles UTF-8 correctly. Test with `📥 Inbox`. |
| Obsidian vault name differs on phone vs desktop | Low | High | Document in config. Obsidian uses directory name by default, which is consistent across sync. |

## Open Questions

- [x] What is Scott's vault name on mobile? The vault directory is `obsidian`, which is the default name.
- [x] Should "Processing..." messages also use HTML parse mode for consistency, or stay plain text? **Stay plain text.** They contain no user-generated content and no links. Switching to HTML adds escaping overhead with no benefit.
- [ ] Should Discord replies include the obsidian:// URI? (Discord mobile may not handle custom URI schemes - needs testing.)

## References

- Obsidian URI scheme docs: https://help.obsidian.md/Extending+Obsidian/Obsidian+URI
- Telegram Bot API parse_mode: https://core.telegram.org/bots/api#formatting-options
- Telegram custom URI handling: custom scheme URIs trigger a confirmation popup
- ChatGPT research on obsidian:// deep links: screenshots in this conversation
- Reply formatter: `src/router.rs:83` (`format_reply`)
- Telegram handler: `src/telegram.rs:121` (message endpoint)
- IngestResult struct: `src/types.rs:78`
- VaultConfig: `src/config.rs:405`
- Pipeline note_path assignment: `src/pipeline.rs:410`, `src/pipeline.rs:755`, etc.
