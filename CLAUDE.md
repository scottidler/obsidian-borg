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
CLI/Telegram/Discord/HTTP
    |
    v
pipeline::process_url()
    |
    +-- hygiene::normalize_url()    # clean + canonicalize
    +-- borg_log::check_duplicate() # dedup via vault-native log
    +-- router::classify_url()      # YouTube vs article vs shorts
    +-- fabric::*                   # fetch, summarize, tag, classify
    +-- markdown::render_note()     # frontmatter + body
    +-- write to vault
    +-- borg_log::append_entry()    # log the ingest
```

## Frontmatter Schema

The current frontmatter spec for ingested notes:

```yaml
---
title: "Note Title"
date: YYYY-MM-DD
day: DayOfWeek
time: "HH:MM"
source: "https://canonical-url"    # was `url:` in obsidian-bookmark era
type: youtube | article            # was `link` in obsidian-bookmark era
method: telegram | discord | http | clipboard | cli
tags:
  - lowercase-hyphenated
uploader: "Channel Name"           # youtube only
duration_min: 10                   # youtube only
---
```

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

## Borg Log and Dedup

- `~/repos/scottidler/obsidian/⚙️ System/Borg Log.md` - append-only markdown table
- Every ingest (success, failure, duplicate) is logged
- Dedup checks this file before processing - only `✅` rows count as duplicates
- `--force` flag on `ingest` bypasses dedup
- The log IS the dedup index - no external database

## Borg Dashboard

- `~/repos/scottidler/obsidian/⚙️ System/Borg Dashboard.md` - Dataview queries
- Shows notes added today, yesterday, this week, this month
- Self-updating via Dataview - obsidian-borg creates it once, never modifies

## Key Conventions

- Tags: always lowercase-hyphenated (`ai-llm` not `AI_LLM`)
- Vault folders use emoji prefixes on disk
- Config secrets can be file paths or env var names (resolved by `config::resolve_secret`)
- Fabric binary at `/home/saidler/go/bin/fabric`
- NEVER use `pip install` - use `pipx`

## Testing

```
cd ~/repos/scottidler/obsidian-borg
cargo test
```

Tests are in each module's `#[cfg(test)] mod tests` block.
