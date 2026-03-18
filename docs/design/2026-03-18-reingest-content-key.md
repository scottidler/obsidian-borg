# Design Document: Reingest - Content Keys and Replace-on-Match Semantics

**Author:** Scott Idler
**Date:** 2026-03-18
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add a stable **content key** to each ingested record and change the default ingest behavior from "skip on duplicate" to "replace on match." When the same canonical input is submitted again, obsidian-borg deletes the old note and reprocesses the content using the current pipeline rules. This enables reprocessing the entire vault after pipeline improvements (new prompts, new frontmatter fields, better routing) without manual intervention.

## Problem Statement

### Background

obsidian-borg currently treats duplicate detection as terminal. When a canonical URL (or text input) matches an existing `✅` ledger entry, the pipeline returns `IngestStatus::Duplicate` and logs a `⏭️` (skipped) row. The `--force` flag bypasses this check but does not clean up the old note - it just creates a new one alongside it.

The trace ID system (`tg-7f3a2c`) is time+pid+counter based - it changes every invocation, making it a correlation ID, not a content identity.

### Problem

1. **No way to update existing notes.** If you improve a Fabric prompt, add a frontmatter field, fix routing rules, or change the summarization model, there's no way to reprocess existing notes through the updated pipeline. The old notes sit in the vault with stale formatting/content indefinitely.

2. **`--force` creates orphans.** When `--force` is used and the reprocessed content produces a different title (e.g., YouTube creator updated it, or a better summarizer extracts a better title), the sanitized filename changes. The old note stays in the vault, the new note appears alongside it. The ledger shows two `✅` rows for the same URL.

3. **No note path in ledger.** The ledger tracks what was ingested (source URL) but not where it landed (vault file path). When a note needs to be replaced, there's no way to find and delete the old file without scanning the vault by frontmatter `source:` field.

4. **Text inputs lack stable identity.** URL inputs are canonicalized (`normalize_url`), giving them a deterministic identity. Text inputs like `"definition: gregarious"` have no canonicalization - whitespace differences or casing would create duplicate records.

### Goals

- Every ingestion gets a deterministic **content key** derived from the canonical input
- Resubmitting the same input replaces the existing note with fresh pipeline output
- The ledger tracks the output file path so old notes can be cleaned up
- Text inputs are canonicalized so minor variations don't create duplicates
- Bulk reingest is possible: reprocess all (or filtered) ledger entries with current pipeline rules

### Non-Goals

- Diffing old vs new notes (just replace wholesale)
- Version history for notes (git handles this)
- Changing the trace ID system (it stays as a per-request correlation ID)
- Changing the ledger file format (markdown table) - we add columns, not change format

## Proposed Solution

### Overview

Three changes that work together:

1. **Content key** - a deterministic identifier derived from the canonical input (URL or normalized text)
2. **Ledger path column** - the ledger records where each note was written
3. **Replace-on-match** - when a content key already exists in the ledger with `✅` status, delete the old note, reprocess, and write the replacement

### Content Key Design

The content key is the **canonical input string itself**, not a hash. For URLs this is the post-normalization canonical URL. For text inputs this is the normalized text.

**Why not hash?** The canonical URL is already a stable, unique string. Hashing it would lose debuggability - you can't look at a hash and know what content it represents. The ledger already stores the source URL in a `Source` column that is human-readable and greppable.

**URL inputs:**
```
raw: https://youtu.be/abc123?si=tracking&t=30s
canonical: https://www.youtube.com/watch?v=abc123
content_key: https://www.youtube.com/watch?v=abc123
```

This already works. `hygiene::normalize_url()` strips tracking params, canonicalizes domains, removes ephemeral params. The result is deterministic for the same underlying content.

**Text inputs:**
```
raw: "  Definition:  Gregarious  "
canonical: "definition: gregarious"
content_key: "definition: gregarious"
```

New: add `hygiene::normalize_text_input()`:
- Trim leading/trailing whitespace
- Collapse internal runs of whitespace to a single space
- Lowercase

This is sufficient because text inputs are short commands (`define: X`, `clarify: X vs Y`), not freeform prose. The normalization handles the realistic variation (extra spaces, casing).

**The content key is never stored as a separate field.** It's the `Source` column value - canonical URL or normalized text. No new column needed for the key itself.

### Ledger Schema Change

Add a `Path` column between `Title` and `Source`:

**Before:**
```
| Date | Time | Method | Status | Title | Source | Domain | Trace |
```

**After:**
```
| Date | Time | Method | Status | Title | Path | Source | Domain | Trace |
```

`Path` is the vault-relative path to the note file (e.g., `notes/some-video-title.md`). For failed or skipped entries, `Path` is `-`.

**Why vault-relative, not absolute?** The vault root is config-driven and could change (e.g., moved to a different machine). Vault-relative paths are portable. The full path is resolved at runtime: `config.vault.root_path + "/" + path`.

**Migration of existing ledger:** Existing rows get `-` for the Path column. A one-time migration adds the column header and pads existing rows. Alternatively, `parse_completed_entries()` handles both old (8-column) and new (9-column) rows by checking column count.

### Replace-on-Match Semantics

Current flow (skip on duplicate):
```
canonical = normalize(input)
if ledger.has_completed(canonical):
    return Duplicate(original_date)
process(input)
write(note)
ledger.append(✅)
```

New flow (replace on match):
```
canonical = normalize(input)
existing = ledger.find_completed(canonical)   // returns (date, path, row_index)
if existing:
    delete vault_root / existing.path          // remove old note file
    mark existing row as 🔄 (Replaced)         // preserve audit trail
process(input)
write(note)
ledger.append(✅, path=vault_relative_path)
```

**New ledger status: `🔄` (Replaced)**
- When a note is replaced, the old `✅` row is updated in-place to `🔄`
- The new `✅` row has the current date, path, and trace ID
- This preserves full history: you can see when something was first ingested and when it was last reprocessed

**What about `--force`?** The semantics change:
- Default behavior (no flag): replace-on-match. Same input = update the note.
- `--force`: reserved for edge cases where you want to bypass ALL checks, including the replace logic. For example, forcing a second note from the same URL into a different domain.

In practice, `--force` becomes rare. The default behavior handles the "redo this" use case naturally.

### Bulk Reingest

New subcommand: `obsidian-borg reingest`

```
obsidian-borg reingest [OPTIONS]

Options:
    --all                  Reingest all completed entries
    --type <TYPE>          Filter by content type (youtube, article, github, etc.)
    --domain <DOMAIN>      Filter by domain (ai, tech, etc.)
    --source <URL>         Reingest a specific URL by content key
    --before <DATE>        Filter entries before date (YYYY-MM-DD)
    --after <DATE>         Filter entries after date (YYYY-MM-DD)
    --dry-run              Show what would be reingested without doing it
    --concurrency <N>      Max parallel reingests (default: 1)
```

Implementation:
1. Parse the ledger for all `✅` entries matching the filter
2. For each entry, extract the `Source` column (content key)
3. Call `process_content()` with replace-on-match semantics
4. The normal pipeline handles deletion of old note, write of new note, ledger update

This is the real payoff. Change a Fabric prompt, update the frontmatter schema, add a new field - then:
```bash
obsidian-borg reingest --all --dry-run       # preview
obsidian-borg reingest --all                 # sweep the vault forward
obsidian-borg reingest --type youtube        # just redo videos
obsidian-borg reingest --after 2026-03-01    # just recent stuff
```

### API Design

**New/changed functions in `ledger.rs`:**

```rust
/// Find the most recent completed entry for a content key.
/// Returns the path and row line number for replacement.
pub fn find_completed(ledger_path: &Path, content_key: &str) -> Result<Option<CompletedEntry>>;

pub struct CompletedEntry {
    pub date: String,
    pub path: String,           // vault-relative note path
    pub line_number: usize,     // for in-place status update
}

/// Mark an existing row as replaced (✅ -> 🔄).
pub fn mark_replaced(ledger_path: &Path, line_number: usize) -> Result<()>;

/// Parse all completed entries, with optional filters.
pub fn query_entries(ledger_path: &Path, filter: &EntryFilter) -> Result<Vec<CompletedEntry>>;

pub struct EntryFilter {
    pub content_type: Option<String>,
    pub domain: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub source: Option<String>,
}
```

**New function in `hygiene.rs`:**

```rust
/// Normalize a text input for use as a content key.
/// Trims whitespace, collapses internal runs, lowercases.
pub fn normalize_text_input(text: &str) -> String;
```

**Changes to `pipeline.rs`:**

`process_url_inner()` dedup section changes from:
```rust
if !force {
    if let Some(original_date) = ledger::check_duplicate(...) {
        return Ok(IngestResult { status: Duplicate ... });
    }
}
```

to:
```rust
if !force {
    if let Some(existing) = ledger::find_completed(&ledger_file, &canonical)? {
        // Delete old note
        let old_path = expand_vault_path(&config.vault.root_path, &existing.path);
        if old_path.exists() {
            std::fs::remove_file(&old_path)?;
            log::info!("[{trace_id}] Deleted old note: {}", old_path.display());
        }
        // Mark old ledger entry as replaced
        ledger::mark_replaced(&ledger_file, existing.line_number)?;
        log::info!("[{trace_id}] Marked ledger row {} as replaced", existing.line_number);
    }
}
// Continue with normal processing...
```

**New CLI subcommand in `cli.rs`:**

```rust
/// Reingest existing entries through the current pipeline
Reingest {
    /// Reingest all completed entries
    #[arg(long)]
    all: bool,
    /// Filter by content type
    #[arg(long, value_name = "TYPE")]
    r#type: Option<String>,
    /// Filter by domain
    #[arg(long)]
    domain: Option<String>,
    /// Reingest a specific URL
    #[arg(long)]
    source: Option<String>,
    /// Filter entries before date (YYYY-MM-DD)
    #[arg(long)]
    before: Option<String>,
    /// Filter entries after date (YYYY-MM-DD)
    #[arg(long)]
    after: Option<String>,
    /// Preview without reingesting
    #[arg(long)]
    dry_run: bool,
    /// Max parallel reingests (default: 1)
    #[arg(long, default_value_t = 1)]
    concurrency: usize,
},
```

### Implementation Plan

**Phase 1: Ledger path column + text normalization**
- Add `Path` column to ledger schema (header + append_entry)
- Handle backward compatibility: old rows without Path parsed as `-`
- Add `normalize_text_input()` to hygiene.rs
- Wire path into `append_entry()` calls in pipeline.rs
- Tests for ledger parsing with/without Path column

**Phase 2: Replace-on-match in pipeline**
- Add `find_completed()` and `mark_replaced()` to ledger.rs
- Add `🔄 Replaced` status to `LedgerStatus`
- Change `process_url_inner()` dedup logic: find -> delete -> mark -> reprocess
- Change `process_text()` (and other handlers) similarly
- Update `check_duplicate()` to not count `🔄` rows as current (only `✅`)
- Tests for replace flow, including title-change orphan prevention

**Phase 3: Reingest subcommand**
- Add `Reingest` variant to `Command` enum
- Add `query_entries()` with filters to ledger.rs
- Implement `run_reingest()` in lib.rs
- Support `--dry-run` for preview
- Sequential processing first; concurrency in a follow-up if needed

**Phase 4: Polish**
- Update dashboard queries if needed (Dataview should handle new status naturally)
- Update `audit` subcommand to detect `🔄` entries whose replacement `✅` is missing
- Add `--concurrency` support for bulk reingest

## Alternatives Considered

### Alternative 1: Hash-based content key
- **Description:** SHA-256 hash of the canonical input, stored as a dedicated column
- **Pros:** Fixed-length, guaranteed unique, no special characters
- **Cons:** Loses human readability. Can't grep the ledger for a URL - you'd need to hash it first. The canonical URL is already unique and deterministic.
- **Why not chosen:** The canonical input IS the key. Adding a hash is indirection without benefit.

### Alternative 2: Keep skip-on-duplicate, add separate `reingest` command only
- **Description:** Don't change default ingest behavior. Add `reingest` that reads ledger, deletes old note, and calls `ingest --force`.
- **Pros:** No change to existing behavior. Lower risk.
- **Cons:** Two different mental models. Users must remember to use `reingest` instead of `ingest` when they want to update. The common case (send same URL again = update) requires a different command.
- **Why not chosen:** The whole point is that sending the same input again should "just work." Having two commands for "process this URL" is confusing.

### Alternative 3: Store note path in frontmatter instead of ledger
- **Description:** Instead of adding a Path column to the ledger, scan vault notes by `source:` frontmatter field to find the existing note.
- **Pros:** No ledger schema change.
- **Cons:** Requires scanning potentially hundreds of files to find a match. Slow, scales poorly. Also fragile - if someone manually edits the source field, the lookup breaks.
- **Why not chosen:** The ledger is the single source of truth for ingest records. Adding the path there keeps the lookup O(1) per entry instead of O(n) per vault file.

### Alternative 4: Content-addressed filenames
- **Description:** Derive filename from the canonical URL (hash or slug of URL), not the content title. Reingest always hits the same file path.
- **Pros:** Guaranteed no orphans on reingest - same URL always writes to same file.
- **Cons:** Filenames become ugly and non-descriptive (e.g., `yt-dQw4w9WgXcQ.md` or `a3f2b8c1.md`). Breaks the current human-readable naming convention. Obsidian graph view and search become harder to use.
- **Why not chosen:** Human-readable filenames are a core design choice. The ledger path column solves the "find old file" problem without sacrificing filename quality.

## Technical Considerations

### Dependencies

No new external dependencies. Uses existing `fs2` for file locking, `chrono` for dates.

### Performance

- `find_completed()` scans the ledger linearly - same as current `check_duplicate()`. The ledger is a few hundred rows; this is sub-millisecond.
- `mark_replaced()` requires reading and rewriting the ledger file (to update a row in place). This needs the exclusive lock. For single ingests this is fine. For bulk reingest, consider batching the marks.
- Bulk reingest with `--concurrency 1` processes sequentially. Each ingest hits external services (Fabric, Jina, yt-dlp), so concurrency beyond ~3-4 would likely bottleneck on those.

### Security

- `std::fs::remove_file()` only deletes within the vault root. Validate that the resolved path is under `config.vault.root_path` before deleting. This prevents a malicious ledger entry from deleting files outside the vault.

### Testing Strategy

- Unit tests for `normalize_text_input()` edge cases (empty, whitespace-only, unicode)
- Unit tests for ledger `find_completed()` and `mark_replaced()` with both old and new format rows
- Integration test: ingest URL, reingest same URL, verify old file deleted and new file written
- Integration test: reingest with title change, verify no orphan
- Test `query_entries()` filters (type, domain, date range)

### Rollout Plan

1. Deploy Phase 1 (ledger path column) - backward compatible, no behavior change
2. Deploy Phase 2 (replace-on-match) - changes default ingest behavior
3. Deploy Phase 3 (reingest subcommand) - new capability
4. Run `obsidian-borg reingest --all --dry-run` to validate
5. Run `obsidian-borg reingest --all` to sweep vault forward

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Old ledger rows lack Path column | Certain | Low | Parser handles both formats; old rows get `-` for Path. On reingest of old entries, fall back to vault scan by `source:` frontmatter. |
| Title change during reingest causes brief "note missing" in Obsidian | Low | Low | Atomic-ish: delete old then immediately write new. Obsidian watches filesystem; delay is sub-second. |
| Bulk reingest hammers external APIs (Fabric, Jina, YouTube) | Medium | Medium | Default concurrency is 1. Rate limiting in fabric.rs already exists. Add progress output so user can Ctrl-C. |
| Ledger rewrite for `mark_replaced()` races with concurrent appends | Low | Medium | Use exclusive file lock (fs2) for the entire read-modify-write cycle. Same pattern as current append. |
| Malicious Path column could delete outside vault | Very Low | High | Validate resolved path starts with vault root before `remove_file()`. |

## Open Questions

- [ ] Should `reingest` support file-based content (images, PDFs, audio)? The ledger has the source for these but the original file may no longer be available. Start with URL-only and expand later.
- [ ] Should the Borg Dashboard show `🔄` entries as a separate category, or just hide them? Probably hide - they're historical, the `✅` replacement is what matters.
- [ ] Should `reingest --all` respect the content quality gate? If a previously successful ingest now fails the quality gate, should it delete the old note or leave it? Probably leave it and log a warning.

## References

- `src/ledger.rs` - current ledger implementation
- `src/pipeline.rs` - current ingest pipeline with dedup logic
- `src/hygiene.rs` - URL normalization and canonicalization
- `src/trace.rs` - trace ID generation (stays as-is)
- `docs/design/2026-03-07-canonicalization-dedup-dashboard.md` - original dedup design
