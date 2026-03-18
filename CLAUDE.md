# obsidian-borg - Claude Code Instructions

## Project Overview

Rust CLI daemon that ingests URLs (via Telegram, Discord, HTTP, clipboard hotkey, CLI) and creates summarized Obsidian markdown notes. Uses Fabric for content fetching, summarization, and tagging.

- **Repo:** `~/repos/scottidler/obsidian-borg/`
- **Vault:** `~/repos/scottidler/obsidian/`
- **Config:** `~/.config/obsidian-borg/obsidian-borg.yml`
- **Example config:** `obsidian-borg.example.yml`
- **Design docs:** `docs/design/`

## Architecture

```
CLI/Telegram/Discord/HTTP/ntfy
    |
    v
notify::Notifier.processing()     # "[trace-id] Processing..." via Telegram
    |
    v
pipeline::process_url()
    |
    +-- hygiene::normalize_url()    # clean + canonicalize
    +-- ledger::check_duplicate()   # dedup via vault-native log
    +-- router::classify_url()      # YouTube vs article vs shorts vs github vs social vs reddit
    +-- fabric::*                   # fetch, summarize, tag, classify domain
    +-- markdown::render_note()     # frontmatter + body
    +-- write to vault (notes/)     # all ingested notes go to notes/
    +-- ledger::append_entry()      # log the ingest
    |
    v
notify::Notifier.result()          # "Saved: Title (2.5s) Tags: ..." via Telegram
```

### Notification Service

All ingestion methods send feedback via a shared `Notifier` (Telegram bot). The notifier accepts an optional `override_chat_id` so Telegram-originated messages reply to the sender, while other methods use a configured default `notification-chat-id`. If Telegram is unavailable, the notifier logs a warning and the pipeline continues silently.

## Frontmatter Schema

The current frontmatter spec for ingested notes (aligned with `system/frontmatter.md` in the vault):

```yaml
---
title: "Note Title"
date: YYYY-MM-DD
source: "https://canonical-url"
type: youtube | article | github | social | reddit | image | pdf | audio | note | vocab | document | code
domain: ai | tech | football | work | writing | music | spanish | knowledge | resources | system
origin: assisted                   # always "assisted" for ingested content
method: telegram | discord | http | clipboard | cli
trace: tg-7f3a2c                   # method-prefixed trace ID
tags:
  - lowercase-hyphenated
creator: "Channel Name"            # youtube/article creator
duration: 10                       # video/audio length in minutes
---
```

Organization is **property-driven** via the `domain` field, not folder-based. All ingested notes go to `notes/`. See `~/repos/scottidler/obsidian/system/frontmatter.md` for the canonical schema reference.

### IMPORTANT: Schema Evolution via Migration

The frontmatter schema WILL change over time. When it does, follow this process:

1. **New notes:** Update `render_note()` in `src/markdown.rs` to emit the new fields.
   This requires a recompile. That's fine - you're already recompiling for the feature.

2. **Existing notes:** Update the `migration` section in the config file:
   ```yaml
   migration:
     field_renames:
       old_field_name: new_field_name
   ```
   Then run:
   ```
   obsidian-borg migrate --dry-run    # review
   obsidian-borg migrate --apply      # write
   cd ~/repos/scottidler/obsidian && git diff  # verify
   ```

3. **The config file is the single source of truth** for migration spec.
   The `migration` section in `~/.config/obsidian-borg/obsidian-borg.yml` defines
   what field renames, transforms, and normalizations `migrate` will perform.
   See the config file for full documentation and examples.

### Do NOT:
- Hardcode migration targets in Rust code
- Write one-off scripts to fix frontmatter - use `migrate`
- Manually bulk-edit frontmatter in the vault - use `migrate --dry-run` then `--apply`
- Forget to update BOTH render_note() AND the migration config when changing schema

## URL Processing Pipeline

```
raw URL
  -> clean_url()           # strip UTM, tracking, ephemeral params (t=, list=, index=)
  -> canonicalize_url()    # normalize domains (youtu.be -> youtube.com, twitter -> x.com)
  = canonical URL          # this is the identity key for dedup and source: field
```

Canonicalization rules are config-driven with built-in defaults. Add new rules
(e.g., old.reddit.com -> reddit.com) in the config without recompiling.

## Borg Ledger and Dedup

- `~/repos/scottidler/obsidian/system/borg-ledger.md` - append-only markdown table
- Columns: Date, Time, Method, Status, Title, Source, Domain, Trace
- Every ingest (success, failure, duplicate) is logged
- Dedup checks this file before processing - only `✅` rows count as duplicates
- `--force` flag on `ingest` bypasses dedup
- The ledger IS the dedup index - no external database

## Borg Dashboard

- `~/repos/scottidler/obsidian/system/borg-dashboard.md` - Dataview queries
- Shows notes added today, yesterday, this week, this month
- Self-updating via Dataview - obsidian-borg creates it once, never modifies

## Key Conventions

- **Filenames:** Always lowercase-hyphenated slugs (e.g., `claude-code-obsidian-guide.md`). No spaces, no uppercase, no underscores. The `title` frontmatter field carries the human-readable display name. `sanitize_filename()` in `src/hygiene.rs` enforces this.
- Tags: always lowercase-hyphenated (`ai-llm` not `AI_LLM`)
- Vault folders: `inbox/`, `daily/`, `notes/`, `system/` (all lowercase, no emojis)
- All ingested notes go to `notes/` - domain is a frontmatter property, not a folder
- Attachments go to `system/attachments/`
- Config secrets can be file paths or env var names (resolved by `config::resolve_secret`)
- Fabric binary at `/home/saidler/go/bin/fabric`
- NEVER use `pip install` - use `pipx`

## Testing

```
cd ~/repos/scottidler/obsidian-borg
cargo test
```

Tests are in each module's `#[cfg(test)] mod tests` block.
