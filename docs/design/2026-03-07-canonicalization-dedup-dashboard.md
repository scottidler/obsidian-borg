# Design Document: URL Canonicalization, Duplicate Prevention, and Borg Dashboard

**Author:** Scott Idler
**Date:** 2026-03-07
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add three interconnected features to obsidian-borg: (1) config-driven URL canonicalization that normalizes domain aliases so `youtu.be/abc` and `youtube.com/watch?v=abc` resolve to the same identity, (2) duplicate prevention using a vault-native Borg Log note as both human-readable history and dedup index, and (3) a Borg Dashboard note with Dataview queries providing exponential-backoff time views of ingested content. Additionally, expand URL hygiene to strip ephemeral query params (`t=`, `list=`, `index=`) and thread an `IngestMethod` enum through all entry points to track how each URL was captured.

## Problem Statement

### Background

obsidian-borg ingests URLs from multiple sources (Telegram, Discord, HTTP POST, clipboard hotkey, CLI) and writes summarized markdown notes into the Obsidian vault at `~/repos/scottidler/obsidian/`. The existing `hygiene::clean_url()` strips UTM tracking parameters, and `router::classify_url()` matches URLs against config-driven regex patterns for type classification.

The vault has rich structure with emoji-prefixed folders, Dataview-compatible frontmatter (`date`, `time`, `source`, `type`, `tags`), and established conventions documented in the vault's `CLAUDE.md`.

### Problem

1. **No duplicate detection.** Sending the same URL twice (or the same video via `youtu.be` vs `youtube.com`) creates duplicate notes. There's no check and no record of what's been ingested.

2. **No URL canonicalization.** `youtu.be/abc123`, `youtube.com/watch?v=abc123`, `m.youtube.com/watch?v=abc123`, and `music.youtube.com/watch?v=abc123` all refer to the same video but are treated as distinct URLs. Similarly, `twitter.com/user/status/123` and `x.com/user/status/123` are the same content.

3. **Ephemeral params pollute identity.** YouTube timestamps (`t=13s`), playlist context (`list=PLxyz`, `index=3`), and radio seeds (`start_radio=1`) are preserved in the cleaned URL. These are transient viewing context, not part of the resource identity, and cause false negatives in any dedup check.

4. **No ingest history.** There's no way to see what was recently added, how it was captured, or whether past ingests succeeded or failed. This information would be valuable both for debugging and for daily use inside Obsidian.

5. **No method tracking.** All ingested notes look identical regardless of whether they came from Telegram, clipboard, or CLI. There's no `method` field in frontmatter.

### Goals

- Canonicalize URLs so the same resource always resolves to the same identity string
- Make canonicalization rules config-driven with sensible built-in defaults
- Strip ephemeral query params (YouTube `t=`, `list=`, `index=`) as part of URL hygiene
- Prevent duplicate ingestion by checking a vault-native log before processing
- Maintain an append-only Borg Log in the vault as both dedup index and human-readable history
- Create a Borg Dashboard with Dataview queries for time-based views (today, yesterday, week, month)
- Track ingest method in note frontmatter (`telegram`, `discord`, `http`, `clipboard`, `cli`)
- Provide `--force` flag to bypass dedup when re-ingestion is intentional

### Non-Goals

- External databases (SQLite, JSONL) for history storage — the vault is the database
- Browser extension or web UI for viewing history
- Automatic re-ingestion of failed URLs
- URL shortener resolution (e.g., `t.co` → actual URL) — could be a future enhancement
- Dedup across different resources that happen to have the same title

## Proposed Solution

### Overview

Three layers, each building on the previous:

```
Layer 1: URL Identity    clean_url() → canonicalize_url() → canonical URL
Layer 2: Dedup + Log     check Borg Log → process if new → append to Borg Log
Layer 3: Dashboard       Dataview queries over note frontmatter
```

### Architecture

#### Layer 1: URL Canonicalization

**Expanded hygiene params** — Add to the existing `TRACKING_PARAMS` list in `hygiene.rs`:

```rust
// YouTube ephemeral context
"t",            // timestamp (t=13s, t=1m30s)
"list",         // playlist ID
"index",        // playlist position
"start_radio",  // YouTube mix seed
"flow",         // YouTube flow parameter
"app",          // app source (app=desktop)
```

**Canonicalization function** — New `canonicalize_url()` in `hygiene.rs`, runs after `clean_url()`:

```rust
pub fn canonicalize_url(url: &str, rules: &[CanonicalRule]) -> String
```

For each rule, test the regex against the URL. If it matches, extract named captures and substitute into the canonical template. First match wins. If no rule matches, return the URL unchanged.

**Config-driven rules with built-in defaults:**

```yaml
canonicalization:
  rules:
    # YouTube Shorts — normalize domain only, keep /shorts/ path
    - name: youtube-shorts-mobile
      match: 'https?://m\.youtube\.com/shorts/(?P<id>[a-zA-Z0-9_-]+)'
      canonical: "https://www.youtube.com/shorts/{id}"

    # YouTube watch — normalize all domain variants to www.youtube.com
    - name: youtube-shortlink
      match: 'https?://youtu\.be/(?P<id>[a-zA-Z0-9_-]+)'
      canonical: "https://www.youtube.com/watch?v={id}"
    - name: youtube-mobile
      match: 'https?://m\.youtube\.com/watch\?v=(?P<id>[a-zA-Z0-9_-]+)'
      canonical: "https://www.youtube.com/watch?v={id}"
    - name: youtube-music
      match: 'https?://music\.youtube\.com/watch\?v=(?P<id>[a-zA-Z0-9_-]+)'
      canonical: "https://www.youtube.com/watch?v={id}"
    - name: youtube-nocookie
      match: 'https?://www\.youtube-nocookie\.com/embed/(?P<id>[a-zA-Z0-9_-]+)'
      canonical: "https://www.youtube.com/watch?v={id}"

    # Twitter/X — normalize to x.com
    - name: twitter-to-x
      match: 'https?://twitter\.com/(?P<path>.*)'
      canonical: "https://x.com/{path}"
    - name: mobile-twitter
      match: 'https?://mobile\.twitter\.com/(?P<path>.*)'
      canonical: "https://x.com/{path}"
```

The code ships with these as `default_canonicalization_rules()`. Config and defaults are merged as follows:

1. Start with the built-in defaults (keyed by `name`)
2. For each config rule: if `name` matches a built-in, replace it; otherwise append it
3. The merged list is the final rule set, evaluated in order (first match wins)

This means adding a new canonicalization (e.g., `old.reddit.com` → `reddit.com`) requires only a config change. Overriding a built-in (e.g., changing the canonical form for YouTube) is also just a config change — use the same `name`.

**Processing order:**

```
raw URL → clean_url() [strip tracking + ephemeral params] → canonicalize_url(rules) → canonical URL
```

The canonical URL is used for:
- Dedup key (check against Borg Log)
- `source:` field in frontmatter
- Display in Borg Log

**Note:** Canonicalization is intentionally lossy. It extracts the resource identity (e.g., video ID) and discards everything else. For YouTube, `https://m.youtube.com/watch?v=abc&someparam=xyz` becomes `https://www.youtube.com/watch?v=abc` — only the video ID survives. This is correct behavior: the canonical form represents "what resource is this?" not "how was it shared?"

**YouTube Shorts are NOT canonicalized to watch URLs.** `youtube.com/shorts/abc` stays as `https://www.youtube.com/shorts/abc` — Shorts are a different content format with different dimensions and rendering. The only Shorts canonicalization is mobile domain normalization: `m.youtube.com/shorts/abc` → `www.youtube.com/shorts/abc`.

**Rule ordering matters.** The Shorts rules must appear before the general YouTube rules in the default list, since `youtu.be/abc` could theoretically match a Shorts video (though in practice `youtu.be` links always point to full videos). First match wins.

#### Layer 2: Duplicate Prevention via Borg Log

**Borg Log note** at `⚙️ System/Borg Log.md` — an append-only markdown table:

```markdown
---
date: 2026-03-07
type: system
tags:
  - obsidian-borg
  - system
---

# Borg Log

All URLs ingested by obsidian-borg. This file is machine-maintained — do not edit the table manually.

| Date | Time | Method | Status | Title | Source | Original | Folder |
|------|------|--------|--------|-------|--------|----------|--------|
| 2026-03-07 | 00:37 | clipboard | ✅ | [[Claude Code Skills Just Got a MASSIVE Upgrade]] | https://www.youtube.com/watch?v=UxfeF4bSBYI | https://youtu.be/UxfeF4bSBYI?si=xyz | 🤖 Tech/AI-LLM |
| 2026-03-07 | 01:15 | telegram | ❌ | — | https://example.com/broken | https://example.com/broken | — |
| 2026-03-07 | 02:00 | telegram | ⏭️ | — | https://www.youtube.com/watch?v=UxfeF4bSBYI | https://m.youtube.com/watch?v=UxfeF4bSBYI&t=45s | — |
```

Status symbols: `✅` completed, `❌` failed, `⏭️` duplicate (skipped).

The Original column records the URL exactly as received (before cleaning/canonicalization). This is invaluable for debugging canonicalization rules — if a URL is incorrectly merged with another, you can see what the original looked like.

**Dedup check flow:**

```
canonical_url = clean + canonicalize
borg_log = read "⚙️ System/Borg Log.md"
if canonical_url found in Source column AND NOT --force:
    return IngestStatus::Duplicate { original_date }
    append ⏭️ row to Borg Log
else:
    process normally
    append ✅ or ❌ row to Borg Log
```

**Reading the log:** Parse the markdown table by splitting lines on `|`. Look for the canonical URL in the Source column (column index 6, 0-based after splitting — accounting for the added Original column). Only rows with `✅` status count as duplicates — failed (`❌`) ingests should be retryable without `--force`. This is O(n) on the number of ingested URLs, which is fine for personal scale (even 10,000 entries is <1MB and parses in microseconds).

**Edge case safety:** URLs cannot contain `|` (not a valid URL character), so pipe-delimited table parsing is safe. Titles are rendered as `[[wikilinks]]` without aliases (no `[[note|alias]]` form) to avoid pipe ambiguity.

**File locking:** Use `fs2::FileExt` for advisory file locking when reading/writing the Borg Log. This prevents race conditions when multiple ingest requests arrive simultaneously (e.g., Telegram + clipboard at the same time).

**Creating the log:** If `⚙️ System/Borg Log.md` doesn't exist on first ingest, create it with the frontmatter and table header.

#### Layer 3: Borg Dashboard

**Borg Dashboard note** at `⚙️ System/Borg Dashboard.md` — contains only Dataview queries, no data:

```markdown
---
date: 2026-03-07
type: system
tags:
  - obsidian-borg
  - system
---

# Borg Dashboard

## 📥 Added Today

```dataview
TABLE WITHOUT ID
  link(file.link, title) as "Title",
  type as "Type",
  method as "Via",
  file.folder as "Folder"
WHERE source != null AND date = date(today)
SORT time DESC
```

## 📅 Yesterday

```dataview
TABLE WITHOUT ID
  link(file.link, title) as "Title",
  type as "Type",
  method as "Via",
  file.folder as "Folder"
WHERE source != null AND date = date(today) - dur(1 day)
SORT time DESC
```

## 📆 This Week

```dataview
TABLE WITHOUT ID
  link(file.link, title) as "Title",
  type as "Type",
  method as "Via",
  file.folder as "Folder"
WHERE source != null AND date >= date(today) - dur(7 day) AND date < date(today) - dur(1 day)
SORT date DESC
```

## 📅 This Month

```dataview
TABLE WITHOUT ID
  link(file.link, title) as "Title",
  type as "Type",
  method as "Via",
  file.folder as "Folder"
WHERE source != null AND date >= date(today) - dur(30 day) AND date < date(today) - dur(7 day)
SORT date DESC
```

## 📊 Stats

```dataview
TABLE WITHOUT ID
  length(rows) as "Count",
  rows.method as "Methods"
WHERE source != null
GROUP BY type
```
```

**Creation:** obsidian-borg creates this file once if it doesn't exist. It never modifies it after creation — the Dataview queries are live and self-updating.

### Data Model

#### New: `IngestMethod` enum

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngestMethod {
    Telegram,
    Discord,
    Http,
    Clipboard,
    Cli,
}

impl std::fmt::Display for IngestMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Telegram => write!(f, "telegram"),
            Self::Discord => write!(f, "discord"),
            Self::Http => write!(f, "http"),
            Self::Clipboard => write!(f, "clipboard"),
            Self::Cli => write!(f, "cli"),
        }
    }
}
```

#### Updated: `IngestStatus` enum

```rust
#[derive(Debug, Default, Serialize, Deserialize)]
pub enum IngestStatus {
    #[default]
    Queued,
    Completed,
    Duplicate { original_date: String },
    Failed { reason: String },
}
```

#### Updated: `IngestResult`

```rust
pub struct IngestResult {
    pub status: IngestStatus,
    pub note_path: Option<String>,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub elapsed_secs: Option<f64>,
    pub folder: Option<String>,
    pub method: Option<IngestMethod>,       // NEW
    pub canonical_url: Option<String>,      // NEW
}
```

#### New: `CanonicalRule` config

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CanonicalRule {
    pub name: String,
    #[serde(rename = "match")]
    pub match_regex: String,  // `match` is reserved in Rust, but config uses `match:`
    pub canonical: String,    // template with {capture_name} placeholders
}
```

#### New: `CanonicalConfig`

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct CanonicalConfig {
    pub rules: Vec<CanonicalRule>,
}
```

Added to the top-level `Config` struct:

```rust
pub struct Config {
    // ... existing fields ...
    pub canonicalization: CanonicalConfig,  // NEW
}
```

The `Default` impl for `CanonicalConfig` returns `default_canonicalization_rules()`, so the built-in rules are always active even with no config.

#### Updated frontmatter

The `method` field is added to note frontmatter via `markdown::render_note()`:

```yaml
---
title: "Some Article"
date: 2026-03-07
day: Friday
time: "14:30"
source: "https://www.youtube.com/watch?v=abc123"
type: youtube
method: telegram
tags:
  - ai
  - rust
---
```

#### New: Borg Log entry

```rust
pub struct BorgLogEntry {
    pub date: String,       // YYYY-MM-DD
    pub time: String,       // HH:MM
    pub method: IngestMethod,
    pub status: BorgLogStatus,  // ✅ ❌ ⏭️
    pub title: Option<String>,
    pub source: String,     // canonical URL
    pub original: String,   // URL as received, before cleaning/canonicalization
    pub folder: Option<String>,
}
```

This struct is only used for formatting the markdown table row — it's not persisted as a separate data structure.

### API Design

#### `hygiene.rs` — new and modified functions

```rust
// Existing — expanded TRACKING_PARAMS list
pub fn clean_url(raw: &str) -> eyre::Result<String>

// New — config-driven canonicalization
pub fn canonicalize_url(url: &str, rules: &[CanonicalRule]) -> String

// New — combined: clean + canonicalize (this is what callers should use)
pub fn normalize_url(raw: &str, rules: &[CanonicalRule]) -> eyre::Result<String> {
    let cleaned = clean_url(raw)?;
    Ok(canonicalize_url(&cleaned, rules))
}
```

#### `router.rs` — updated to use `normalize_url`

`classify_url()` currently calls `hygiene::clean_url()` on line 40. This changes to `hygiene::normalize_url()`, passing the canonicalization rules from config. The `UrlMatch.url` field will contain the canonical URL from this point forward.

```rust
// Before (router.rs:40)
let cleaned = hygiene::clean_url(raw_url)?;

// After
let cleaned = hygiene::normalize_url(raw_url, &config_rules)?;
```

However, `classify_url()` currently only takes `&[LinkConfig]`. Rather than changing its signature to also take `&[CanonicalRule]`, we normalize the URL **before** calling `classify_url()` — in `pipeline::process_url()`. This keeps `classify_url` focused on classification.

**Important:** `classify_url()` currently calls `hygiene::clean_url()` internally (router.rs:40). Since the URL is already normalized before being passed in, this internal `clean_url()` call becomes a no-op (cleaning an already-clean URL). We should remove the `clean_url()` call from `classify_url()` and have it accept a pre-normalized URL. This avoids double-cleaning and makes the data flow explicit.

#### `pipeline.rs` — updated signature and flow

```rust
// Before
pub async fn process_url(url: &str, tags: Vec<String>, config: &Config) -> IngestResult

// After
pub async fn process_url(
    url: &str,              // raw URL as received
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> IngestResult
```

The inner function flow becomes:

```rust
async fn process_url_inner(...) -> Result<IngestResult> {
    // Step 1: Normalize URL (clean + canonicalize)
    let canonical = hygiene::normalize_url(url, &config.canonicalization.rules)?;

    // Step 2: Dedup check (skip if --force)
    if !force {
        let log_path = borg_log::log_path(config);
        if let Some(original_date) = borg_log::check_duplicate(&log_path, &canonical)? {
            borg_log::append_entry(&log_path, &BorgLogEntry { status: Skipped, .. })?;
            return Ok(IngestResult { status: Duplicate { original_date }, .. });
        }
    }

    // Step 3: Classify (uses canonical URL)
    let url_match = router::classify_url(&canonical, &config.links)?;

    // Step 4: Process (existing pipeline)
    // ... fetch, summarize, tag, route, write note ...

    // Step 5: Log success
    borg_log::append_entry(&log_path, &BorgLogEntry { status: Completed, .. })?;
}
```

#### `borg_log.rs` — new module

```rust
// Check if canonical URL exists in log, return the date if found
pub fn check_duplicate(log_path: &Path, canonical_url: &str) -> eyre::Result<Option<String>>

// Append a row to the Borg Log table
pub fn append_entry(log_path: &Path, entry: &BorgLogEntry) -> eyre::Result<()>

// Create the Borg Log file with frontmatter and header if it doesn't exist
pub fn ensure_log_exists(log_path: &Path) -> eyre::Result<()>

// Resolve the Borg Log path from config
pub fn log_path(config: &Config) -> PathBuf
```

#### `dashboard.rs` — new module

```rust
// Create the Borg Dashboard file if it doesn't exist
pub fn ensure_dashboard_exists(dashboard_path: &Path) -> eyre::Result<()>

// Resolve the dashboard path from config
pub fn dashboard_path(config: &Config) -> PathBuf
```

#### `markdown.rs` — updated `NoteContent`

Add `method: Option<IngestMethod>` to the `NoteContent` struct and render it in frontmatter:

```rust
pub struct NoteContent {
    // ... existing fields ...
    pub method: Option<IngestMethod>,  // NEW
}
```

In `render_note()`, after the `type:` line:
```rust
if let Some(method) = &note.method {
    fm.push_str(&format!("method: {method}\n"));
}
```

#### Caller changes — threading `IngestMethod`

Each entry point passes the appropriate method:

| Call site | Method | How determined |
|-----------|--------|----------------|
| `telegram.rs:33` | `Telegram` | Hardcoded — it's the Telegram handler |
| `discord.rs:29` | `Discord` | Hardcoded — it's the Discord handler |
| `routes.rs:19` | `Http` | Hardcoded — it's the HTTP endpoint |
| `lib.rs:132` (`run_ingest`) | `Clipboard` or `Cli` | `if clipboard { Clipboard } else { Cli }` — the `clipboard` bool is already in scope |

#### CLI changes

```rust
// Updated Ingest command
Some(Command::Ingest { url, clipboard, tags, force }) => { ... }

// New History command — reads and displays Borg Log
Some(Command::History(opts)) => { ... }

// New Migrate command — one-time vault frontmatter normalization
Some(Command::Migrate { dry_run, apply }) => { ... }
```

`--force` on `ingest` bypasses dedup. `history` is a convenience command that reads `⚙️ System/Borg Log.md` and prints a filtered/formatted view to the terminal — the same data that's in the vault, but useful for quick CLI checks without opening Obsidian.

#### `borg_log.rs` — path resolution

`log_path()` derives from the vault root in config:

```rust
pub fn log_path(config: &Config) -> PathBuf {
    let root = expand_tilde(&config.vault.root_path);
    root.join("⚙️ System").join("Borg Log.md")
}
```

Similarly for `dashboard_path()`:

```rust
pub fn dashboard_path(config: &Config) -> PathBuf {
    let root = expand_tilde(&config.vault.root_path);
    root.join("⚙️ System").join("Borg Dashboard.md")
}
```

### Implementation Plan

#### Phase 0: Vault Migration

The vault contains ~846 notes from the old obsidian-bookmark era and ~71 notes from early obsidian-borg. These need normalization before the dedup system can work reliably.

**Audit findings:**

| Pattern | Count | Era |
|---------|-------|-----|
| `url:` field (no `source:`) | ~846 | obsidian-bookmark |
| `source:` field | ~71 | obsidian-borg |
| `type: link` | ~846 | obsidian-bookmark |
| `type: youtube` | ~40 | obsidian-borg |
| `type: article` | 0 | — |
| `method:` field | 0 | — |
| Inline tags (`#old, #philosophy`) | unknown | oldest notes |

**Design decision: config-driven migration spec.**

The frontmatter schema will evolve over time. Rather than hardcoding the target spec in the `migrate` command, the migration rules live in config. The `render_note()` function in `markdown.rs` stays hardcoded (it changes with features, and you're already recompiling for those). But `migrate` reads its spec from config, making it **rerunnable after future schema changes** without recompiling.

```yaml
# Migration spec — tells `obsidian-borg migrate` what "correct" looks like.
# This is the SINGLE SOURCE OF TRUTH for schema evolution.
#
# HOW TO USE (for future schema changes):
#   1. Add/update entries in field_renames and field_transforms below
#   2. Run: obsidian-borg migrate --dry-run    (review changes)
#   3. Run: obsidian-borg migrate --apply       (write changes)
#   4. Review: cd ~/repos/scottidler/obsidian && git diff
#   5. Commit the vault changes
#   6. Update render_note() in markdown.rs to match (for new notes)
#
# EXAMPLE: Renaming 'source' to 'origin' in the future:
#   field_renames:
#     url: origin        # obsidian-bookmark era
#     source: origin     # obsidian-borg era
#   Then re-run migrate --apply.

migration:
  field_renames:
    url: source            # obsidian-bookmark used `url:`, we use `source:`

  field_transforms:
    source: canonicalize   # run URL value through canonicalization rules
    type: reclassify       # `link` → `youtube` or `article` based on URL
    tags: normalize        # inline `#tag, #tag` → YAML list format

  title_fallback: true     # if `title:` missing, derive from H1 or filename

  seed_borg_log: true      # backfill Borg Log with existing notes' URLs

  skip_folders:
    - "⚙️ System"
    - "📆 Daily"
    - "templates"
```

**Migration script:** `obsidian-borg migrate` — a CLI subcommand that scans the vault and conforms frontmatter to the current spec:

1. **Field renames** — For each entry in `field_renames`, if the old field exists and the new field doesn't, rename it. If both exist, keep the new one (newer is more likely correct).
2. **`source: canonicalize`** — Run the URL value through the current canonicalization rules.
3. **`type: reclassify`** — If `type: link`, inspect `source:` URL: YouTube domains → `type: youtube`, everything else → `type: article`.
4. **`tags: normalize`** — Convert inline tags (`tags: #old, #philosophy`) to list format. Strip `#` prefixes. Sanitize via `hygiene::sanitize_tag()`.
5. **`title_fallback`** — If `title:` is missing, derive from the first H1 heading (`# Title`) in the body, or from the filename.
6. **`seed_borg_log`** — For every note with a `source:` URL, append a `✅` row to the Borg Log with `method: migration` and the note's `date:` field. This backfills the dedup index so existing URLs aren't re-ingested.
7. **`method: migration`** — Do NOT add `method:` to old notes' frontmatter (they weren't ingested via any current method). The Borg Log entry records `migration` as the method.

**Safety:**
- Dry-run mode by default (`--dry-run`). Shows what would change without writing.
- `--apply` to actually write changes.
- Git diff review before committing — the vault is a git repo.
- Skips notes in folders listed in `skip_folders`.
- Idempotent — running migrate twice produces the same result.

**Reusability:** This is not a one-shot script. When the schema evolves in the future:
1. Update `migration.field_renames` in config (add old→new mappings)
2. Re-run `obsidian-borg migrate --apply`
3. All existing notes conform to the new spec

#### Phase 1: URL Identity (hygiene + canonicalization)

1. Expand `TRACKING_PARAMS` with YouTube ephemeral params (`t`, `list`, `index`, `start_radio`, `flow`, `app`)
2. Add `CanonicalRule` and `CanonicalConfig` to `config.rs` with `default_canonicalization_rules()`
3. Implement `canonicalize_url()` and `normalize_url()` in `hygiene.rs`
4. Update `router::classify_url()` to use `normalize_url()` instead of `clean_url()`
5. Update `obsidian-borg.example.yml` with canonicalization section and examples
6. Update `~/.config/obsidian-borg/obsidian-borg.yml` with real rules
7. Tests for all YouTube variants, twitter/x, and custom rules

#### Phase 2: IngestMethod threading

1. Add `IngestMethod` enum to `types.rs`
2. Update `pipeline::process_url()` signature to accept `IngestMethod`
3. Thread method through all callers:
   - `telegram.rs` → `IngestMethod::Telegram`
   - `discord.rs` → `IngestMethod::Discord`
   - `routes.rs` (HTTP) → `IngestMethod::Http`
   - `lib.rs::run_ingest()` → `IngestMethod::Clipboard` or `IngestMethod::Cli`
4. Add `method:` field to frontmatter in `markdown::render_note()`
5. Update `IngestResult` to include `method` and `canonical_url`

#### Phase 3: Borg Log + Dedup

1. Add `borg_log.rs` module with `ensure_log_exists()`, `check_duplicate()`, `append_entry()`
2. Add `Duplicate` variant to `IngestStatus`
3. Add `--force` flag to `ingest` CLI command
4. Integrate into `pipeline::process_url()`: check log before processing, append after
5. Update `router::format_reply()` to handle `Duplicate` status
6. Handle file locking with `fs2` crate for concurrent access
7. Create Borg Log on first ingest if missing
8. Tests for dedup logic, log parsing, concurrent access

#### Phase 4: Borg Dashboard

1. Add `dashboard.rs` module with `ensure_dashboard_exists()`
2. Write dashboard template with Dataview queries
3. Create dashboard on daemon startup if missing
4. Add `history` CLI subcommand (reads Borg Log, prints to terminal)

## Alternatives Considered

### Alternative 1: SQLite / JSONL for history

- **Description:** Store ingest history in `~/.local/share/obsidian-borg/history.db` or `.jsonl`
- **Pros:** Fast indexed lookups, proper concurrent access, time-range queries via SQL
- **Cons:** History lives outside the vault — invisible in Obsidian, requires separate tooling to view, adds `rusqlite` dependency
- **Why not chosen:** The vault IS the database. The user consumes this data inside Obsidian with Dataview. External storage creates a parallel system that doesn't serve the primary use case.

### Alternative 2: Scan vault frontmatter for dedup

- **Description:** On each ingest, scan all `.md` files in the vault for a matching `source:` field
- **Pros:** No additional file to maintain, uses existing data
- **Cons:** O(n) file reads across potentially thousands of notes, slow on large vaults, no record of failed ingests
- **Why not chosen:** Single-file log is faster (one file read vs. thousands) and captures failed/duplicate attempts that wouldn't have notes.

### Alternative 3: Dedup via filename collision

- **Description:** If a file with the same sanitized filename already exists, skip
- **Pros:** Zero additional infrastructure
- **Cons:** Different URLs can produce the same title, same URL can produce different titles over time
- **Why not chosen:** Filename is not a reliable identity key. URL is.

### Alternative 4: Hardcoded canonicalization only

- **Description:** Implement YouTube/twitter canonicalization directly in Rust with no config
- **Pros:** Simpler implementation, fewer moving parts
- **Cons:** Adding new canonicalization rules (reddit, medium, etc.) requires recompile and redeploy
- **Why not chosen:** Config-driven approach is trivially more work but dramatically more maintainable. The user runs this as a systemd daemon — config reload vs binary rebuild is a significant UX difference.

## Technical Considerations

### Dependencies

**New crate:** `fs2` for advisory file locking on the Borg Log. Lightweight, well-maintained, no transitive dependencies. Already in the Rust ecosystem for this exact use case.

All other functionality uses existing dependencies (`regex`, `chrono`, `serde_yaml`, `url`).

### Performance

- **Canonicalization:** Compiling N regexes on startup (typically 6-8 rules). Could cache compiled regexes via `LazyLock` or compile once at config load. Negligible cost.
- **Dedup check:** Reading and scanning one markdown file. At 10,000 entries the file is ~1MB. String search is sub-millisecond.
- **Log append:** Single file write with advisory lock. Negligible.

### Security

- Borg Log is world-readable in the vault (same as all other notes). No secrets are stored.
- Canonicalization rules use regex — invalid regex in config should produce a clear error at startup, not a panic.
- File locking is advisory only. A misbehaving process could still corrupt the log, but this is acceptable for a single-user personal tool.

### Testing Strategy

- **Unit tests:** `canonicalize_url()` with all YouTube variants, twitter/x, custom rules, no-match passthrough, overlapping rules
- **Unit tests:** `clean_url()` with new ephemeral params (`t=`, `list=`, `index=`)
- **Unit tests:** `check_duplicate()` with matching URL, non-matching URL, empty log, malformed log
- **Unit tests:** `append_entry()` creates valid markdown table row
- **Unit tests:** Config deserialization with canonicalization rules, default merging
- **Integration tests:** Full pipeline with duplicate URL → `IngestStatus::Duplicate`
- **Integration tests:** Full pipeline with `--force` flag → re-processes duplicate

### Rollout Plan

1. Ship Phase 1 (canonicalization + expanded hygiene) — no behavioral change for non-duplicate URLs
2. Ship Phase 2 (method threading) — adds `method:` to new notes, doesn't affect existing
3. Ship Phase 3 (Borg Log + dedup) — creates `⚙️ System/Borg Log.md` on first ingest
4. Ship Phase 4 (dashboard) — creates `⚙️ System/Borg Dashboard.md` on daemon startup
5. Run Phase 0 (migration) — normalize existing notes, seed Borg Log. Runs AFTER Phase 3 so the log format exists.
6. Existing notes without `method:` field work fine — Dataview queries use `WHERE source != null` not `WHERE method != null`

**Note:** Phase 0 runs after Phase 3 even though it's numbered 0 — it needs the Borg Log infrastructure to exist before it can seed entries. It's numbered 0 because conceptually it addresses pre-existing state.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Borg Log gets corrupted (manual edit, merge conflict) | Low | Medium | Log parser is lenient — skips malformed rows. Add a `history --repair` command later if needed. |
| Regex in config is invalid | Low | Medium | Validate all rules at config load time, fail fast with clear error message. |
| Concurrent writes to Borg Log (Telegram + clipboard at same time) | Medium | Low | Advisory file locking via `fs2`. Worst case: duplicate row, not data loss. |
| Canonical URL changes over time (e.g., twitter.com fully sunsets) | Low | Low | Config-driven rules can be updated without recompile. |
| Dataview plugin not installed | Low | Medium | Dashboard note is just markdown with code blocks — it won't break, it just won't render the tables. Dashboard file includes a note about requiring Dataview. |
| Large Borg Log slows dedup check | Very Low | Low | Even 50,000 entries is ~5MB. Simple string search is still sub-10ms. Can add indexing later if needed. |
| Obsidian has Borg Log open when daemon writes | Medium | Low | Obsidian watches for file changes and reloads. Append-only writes are safe — Obsidian handles this gracefully. No data loss risk. |
| YouTube Shorts falsely canonicalized to watch URL | Low | Medium | Shorts rules ordered before general YouTube rules. Shorts paths (`/shorts/`) are distinct from watch paths (`/watch?v=`). |
| Force flag unavailable for Telegram/Discord users | Low | Low | Bot users can't pass `--force`. If they need to re-ingest, they use the CLI. This is acceptable — re-ingestion is rare. |
| Migration corrupts notes | Low | High | Dry-run by default. Git diff review before commit. Only touches frontmatter YAML block, never body content. Skips system/template/daily notes. |
| Migration misclassifies `type: link` as article vs youtube | Low | Medium | Simple URL-based heuristic (contains `youtube.com`/`youtu.be` → youtube, else article). Covers 99%+ of cases. |

## Open Questions

- [x] Where does Borg Dashboard live? → `⚙️ System/Borg Dashboard.md` (consistent with vault conventions, user can pin/star it)
- [x] Should the Borg Log record the original URL in addition to the canonical URL? → Yes. Added Original column. Essential for debugging canonicalization rules.
- [ ] Should `history` CLI subcommand support `--today`, `--week`, `--month` filters? (Nice to have but the primary view is inside Obsidian)
- [ ] Should failed ingests be retried automatically on next daemon restart? (Marked as non-goal but worth flagging)

## References

- Existing design: `docs/design/2026-03-06-smart-ingestion-pipeline.md`
- Vault conventions: `~/repos/scottidler/obsidian/CLAUDE.md`
- Current URL hygiene: `src/hygiene.rs`
- Current pipeline: `src/pipeline.rs`
- Dataview docs: https://blacksmithgu.github.io/obsidian-dataview/
