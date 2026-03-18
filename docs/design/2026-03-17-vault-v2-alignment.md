# Design Document: Vault v2 Alignment

**Author:** Scott Idler
**Date:** 2026-03-17
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

The Obsidian vault has been migrated from emoji-prefixed, folder-based organization to a flat, property-driven architecture. obsidian-borg still targets the old structure - emoji paths, `folder` semantics, and deprecated frontmatter fields. This design doc covers every change needed to align obsidian-borg with the vault's new schema, folder layout, and routing model.

## Problem Statement

### Background

The vault migration (6 commits, 2026-03-17) restructured `~/repos/scottidler/obsidian/` from:
- Emoji-prefixed topic folders (`📥 Inbox/`, `🤖 Tech/`, `🧠 Knowledge/`, `⚙️ System/`, etc.)
- Folder-based organization (a note's topic = which folder it lives in)
- Legacy frontmatter fields (`day`, `time`, `author`, `uploader`, `duration_min`, `trace_id`)

To:
- Four flat directories: `inbox/`, `daily/`, `notes/`, `system/`
- Property-driven organization (`domain` frontmatter field replaces folders)
- Clean frontmatter schema documented in `system/frontmatter.md`

obsidian-borg was not updated during the vault migration. It will crash or write to nonexistent directories if run.

### Problem

obsidian-borg has three categories of breakage:

1. **Hard failure** - Writes to `⚙️ System/`, `📥 Inbox/`, emoji topic folders that no longer exist
2. **Wrong schema** - Emits deprecated fields (`day`, `time`, `trace_id`, `author`, `uploader`, `duration_min`), missing required fields (`domain`, `origin`)
3. **Wrong semantics** - Routes notes to folders by topic instead of writing all to `notes/` with a `domain` property

### Goals

- obsidian-borg writes notes to the correct directories (`notes/`, `system/`)
- Frontmatter matches the canonical schema in `system/frontmatter.md`
- The `domain` field replaces folder-based routing throughout the codebase
- Ledger and dashboard paths resolve correctly
- All tests pass against the new schema
- Config file updated with correct paths

### Non-Goals

- Changing the Fabric patterns themselves (they return whatever they return; we rename the field on our side)
- Adding new content types or ingestion methods
- Vault-side changes (the vault migration is complete)
- Changing the dedup/ledger logic (column rename only)
- Migration subcommand changes (the migration is done)

## Proposed Solution

### Overview

A methodical rename-and-simplify across the codebase, organized into four phases:

1. **Phase 1: Config, paths, and struct renames** - Fix emoji paths, rename `folder` -> `domain` in all struct definitions, update consts
2. **Phase 2: Call sites and pipeline logic** - Fix all references to renamed fields, simplify `resolve_destination()`, update all `process_*` functions
3. **Phase 3: Frontmatter schema** - Update `render_note()` and `NoteContent` to match `system/frontmatter.md`
4. **Phase 4: Tests** - Update all assertions and test data to match new schema

### Data Model

#### NoteContent (src/markdown.rs)

Add `domain` field:

```rust
pub struct NoteContent {
    pub title: String,
    pub source_url: Option<String>,
    pub asset_path: Option<String>,
    pub tags: Vec<String>,
    pub summary: String,
    pub content_type: ContentType,
    pub embed_code: Option<String>,
    pub method: Option<IngestMethod>,
    pub trace_id: Option<String>,  // internal field name stays trace_id
    pub domain: String,            // NEW - "ai", "tech", "football", etc.
}
```

#### LedgerEntry (src/ledger.rs)

```rust
pub struct LedgerEntry {
    pub date: String,
    pub time: String,
    pub method: IngestMethod,
    pub status: LedgerStatus,
    pub title: Option<String>,
    pub source: String,
    pub domain: Option<String>,    // was: folder
    pub trace_id: Option<String>,
}
```

Ledger table header: `| Date | Time | Method | Status | Title | Source | Domain | Trace |`

#### ClassificationResult (src/fabric.rs)

```rust
pub struct ClassificationResult {
    pub domain: String,            // was: folder
    pub confidence: f64,
    pub suggested_tags: Vec<String>,
}
```

Note: The Fabric `obsidian_classify` pattern may still return JSON with a `folder` key. Use `#[serde(alias = "folder")]` on the `domain` field for backward compatibility until the pattern is updated.

#### UrlMatch (src/router.rs)

```rust
pub struct UrlMatch {
    pub url: String,
    pub link_name: String,
    pub domain: String,            // was: folder
    pub width: usize,
    pub height: usize,
}
```

#### IngestResult (src/types.rs)

```rust
pub struct IngestResult {
    // ... other fields ...
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "folder")]
    pub domain: Option<String>,    // was: folder
    // ...
}
```

Note: `IngestResult` is serialized to JSON for the HTTP API and Telegram replies. The `alias = "folder"` handles any external consumers still sending the old field name. The serialized output will use `domain`.

#### Config structs (src/config.rs)

```rust
// LinkConfig
pub struct LinkConfig {
    pub name: String,
    pub regex: String,
    pub resolution: String,
    pub domain: String,            // was: folder
}

// RoutingConfig
// Note: uses #[serde(rename_all = "kebab-case")] so field names map to kebab-case YAML keys.
// fallback_domain -> fallback-domain in YAML. Add #[serde(alias = "fallback-folder")] for
// backward compat with existing config files that still have the old key name.
pub struct RoutingConfig {
    pub confidence_threshold: f64,
    #[serde(alias = "fallback-folder")]
    pub fallback_domain: String,   // was: fallback_folder, default: "inbox"
    pub research_date_subfolder: bool,  // REMOVE - no longer applicable
    pub routes: Vec<TopicRoute>,
}

// TopicRoute
pub struct TopicRoute {
    pub domain: String,            // was: folder
    pub keywords: Vec<String>,
}

// TextCaptureConfig
pub struct TextCaptureConfig {
    pub vocab_domain: String,      // was: vocab_folders HashMap, default: "knowledge"
    pub code_domain: String,       // was: code_folder, default: "tech"
}
```

### Path Changes

Every hardcoded emoji path in the codebase:

| File | Line | Old | New |
|------|------|-----|-----|
| `src/ledger.rs` | 55 | `root.join("⚙️ System").join("borg-ledger.md")` | `root.join("system").join("borg-ledger.md")` |
| `src/dashboard.rs` | 80 | `root.join("⚙️ System").join("borg-dashboard.md")` | `root.join("system").join("borg-dashboard.md")` |
| `src/assets.rs` | 34 | `vault_root.join("⚙️ System/attachments")` | `vault_root.join("system/attachments")` |
| `src/assets.rs` | 43 | `format!("⚙️ System/attachments/...")` | `format!("system/attachments/...")` |
| `src/config.rs` | 223-225 | Emoji vocab folders | Remove HashMap, single `vocab_domain` string |
| `src/config.rs` | 228 | `"🤖 Tech/snippets"` | Remove, single `code_domain` string |
| `src/config.rs` | 350 | `fallback_folder: "Inbox"` | `fallback_domain: "inbox"` |
| `src/config.rs` | 488 | `inbox_path: "~/obsidian-vault/Inbox"` | `inbox_path: "~/obsidian-vault/inbox"` |

### Frontmatter Schema Changes

`render_note()` in `src/markdown.rs` must produce:

```yaml
---
title: "Note Title"
date: 2026-03-17
source: "https://canonical-url"
type: youtube
domain: ai
origin: assisted
method: telegram
trace: tg-7f3a2c
tags:
  - ai
  - llm
creator: "Channel Name"
duration: 10
---
```

Changes from current output:

| Change | Detail |
|--------|--------|
| **Drop** `day` | Derivable from date |
| **Drop** `time` | Low value per schema |
| **Add** `domain` | Required, from classifier or fallback |
| **Add** `origin` | Always `assisted` for ingested content |
| **Rename** `trace_id` -> `trace` | Field name simplification |
| **Rename** `author` -> `creator` | Field merge. Also rename `FrontmatterConfig.default_author` to `default_creator` |
| **Rename** `uploader` -> `creator` | Field merge (YouTube) |
| **Rename** `duration_min` -> `duration` | Field rename |

Additionally, `FrontmatterConfig.default_author` (config.rs:322) must be renamed to `default_creator` with `#[serde(alias = "default-author")]` for YAML backward compat.

### Routing Simplification

**Before:** `resolve_destination()` maps folder strings to filesystem paths (3-tier routing).

**After:** All ingested notes go to `notes/`. The domain value is written to frontmatter only.

```rust
fn resolve_destination(root_path: &str) -> PathBuf {
    expand_tilde(root_path).join("notes")
}
```

The 3-tier classification logic stays - it still determines the `domain` value. But that value goes into the `NoteContent.domain` field, not into a filesystem path.

Special cases:
- **Vocab notes**: Go to `notes/` with `domain: knowledge` (or `domain: spanish`)
- **Code snippets**: Go to `notes/` with `domain: tech`
- **Failed/skipped ingests**: No note written, ledger gets `domain: None`

The `inbox_path` config field is kept for now but only used as the path for `inbox/` directory (untriaged content). The `resolve_destination()` function no longer references it - ingested content always goes to `notes/`.

### Ledger Frontmatter Update

The `LEDGER_FRONTMATTER` const (src/ledger.rs:36-50) needs updating to match the vault schema:

```rust
const LEDGER_FRONTMATTER: &str = r#"---
title: Borg Ledger
date: {date}
type: system
domain: system
origin: authored
tags:
  - obsidian-borg
  - system
---

# Borg Ledger

All URLs ingested by obsidian-borg. This file is machine-maintained - do not edit the table manually.

See also: [[borg-dashboard]]

| Date | Time | Method | Status | Title | Source | Domain | Trace |
|------|------|--------|--------|-------|--------|--------|-------|
"#;
```

Note: The existing ledger file in the vault already has the correct format (title, domain, origin fields present, Domain column header). The `ensure_ledger_exists` function only writes this if the file is missing, so this is a safety net, not an overwrite.

### Dashboard Content Update

The `DASHBOARD_CONTENT` const (src/dashboard.rs:6-75) has Dataview queries using `file.folder as "Folder"`. Update to use the `domain` frontmatter property:

```
domain as "Domain"
```

Same caveat as ledger - the vault's dashboard already has correct content. This is for the create-if-missing path.

### Config File Update

`~/.config/obsidian-borg/obsidian-borg.yml` changes:

```yaml
vault:
  root-path: ~/repos/scottidler/obsidian/
  inbox-path: ~/repos/scottidler/obsidian/inbox    # was: .../📥 Inbox
  vault-name: obsidian

routing:
  confidence-threshold: 0.6
  fallback-domain: inbox    # was: fallback-folder: "📥 Inbox"

migration:
  skip-folders:
    - system      # was: "⚙️ System"
    - daily       # was: "📆 Daily"
    - templates   # was: "templates"
```

### Implementation Plan

**Important:** These phases are tightly coupled - renaming struct fields in one file breaks all callers. The phases are logical groupings for understanding, but implementation should proceed as a single atomic change per commit. The recommended approach: implement all phases, fix all compiler errors, then run tests.

Alternatively, if you want incremental compiles, work bottom-up: config structs first, then the modules that use them, then pipeline.rs (which uses everything).

#### Phase 1: Config, Paths, and Struct Renames

Files: `src/config.rs`, `src/ledger.rs`, `src/dashboard.rs`, `src/assets.rs`, `src/fabric.rs`, `src/router.rs`, `src/types.rs`, `obsidian-borg.example.yml`

This phase changes struct definitions and constants. Everything that references these structs will break until Phase 2 fixes the call sites.

1. **config.rs**: Rename `fallback_folder` -> `fallback_domain` (add `#[serde(alias = "fallback-folder")]`), rename `LinkConfig.folder` -> `domain`, rename `TopicRoute.folder` -> `domain`, simplify `TextCaptureConfig` (HashMap -> single `vocab_domain`/`code_domain` strings), rename `default_author` -> `default_creator` (add `#[serde(alias = "default-author")]`), update default `inbox_path`
2. **config.rs**: Remove emoji paths from `TextCaptureConfig` defaults
3. **ledger.rs**: Rename `LedgerEntry.folder` -> `domain`, update `LEDGER_FRONTMATTER` const (add title/domain/origin, fix em dash, rename Folder column to Domain), update `ledger_path()` to use `"system"`, update comment on line 94, rename `folder_display` -> `domain_display`
4. **dashboard.rs**: Update `dashboard_path()` to use `"system"`, update `DASHBOARD_CONTENT` (`file.folder as "Folder"` -> `domain as "Domain"`)
5. **assets.rs**: Replace `"⚙️ System/attachments"` with `"system/attachments"` (lines 34, 43)
6. **fabric.rs**: Rename `ClassificationResult.folder` -> `domain` (add `#[serde(alias = "folder")]`)
7. **router.rs**: Rename `UrlMatch.folder` -> `domain`
8. **types.rs**: Rename `IngestResult.folder` -> `domain` (add `#[serde(alias = "folder")]`)

#### Phase 2: Call Sites and Pipeline Logic

Files: `src/pipeline.rs`, `src/telegram.rs`, `src/discord.rs`, `src/router.rs`, `src/markdown.rs`

Fix every reference to the renamed fields. This is the largest phase by line count.

1. **pipeline.rs**: Rename all `folder` local variables to `domain` (~40 occurrences). Update all `LedgerEntry { folder: ... }` to `domain: ...`. Update all `IngestResult { folder: ... }` to `domain: ...`. Update log messages ("folder: {}" -> "domain: {}").
2. **pipeline.rs**: Simplify `resolve_destination()` - always return `root/notes/`. Remove `inbox_path` and `routing` parameters. Remove `research_date_subfolder` logic.
3. **pipeline.rs**: Simplify `resolve_vocab_folder()` -> `resolve_vocab_domain()` - returns a domain string ("knowledge", "spanish") instead of a folder path. Uses `TextCaptureConfig.vocab_domain` as default.
4. **pipeline.rs**: Update all `process_*` functions: classify domain, pass it to `NoteContent.domain`, use simplified `resolve_destination()` for path.
5. **router.rs**: Update `format_reply()` line 107-111: `"Folder: {f}"` -> `"Domain: {f}"`. Update `classify_url()` to use `.domain` instead of `.folder`.
6. **telegram.rs**: Update `IngestResult` field references from `.folder` to `.domain`.
7. **discord.rs**: Same as telegram.

#### Phase 3: Frontmatter Schema

Files: `src/markdown.rs`

1. Add `domain: String` field to `NoteContent` struct
2. In `render_note()`: drop `day` and `time` from format string (line 94)
3. In `render_note()`: add `domain: {domain}` and `origin: assisted` after `type:` line
4. In `render_note()`: change `trace_id:` output to `trace:` (line 111)
5. In `render_note()`: change `author:` output to `creator:` (line 118)
6. In `render_note()`: change `uploader:` to `creator:` and `duration_min:` to `duration:` in YouTube match arm (line 130-131)
7. In `render_note()`: change `duration_min:` to `duration:` in Audio match arm (line 139)

#### Phase 4: Tests

Files: All `#[cfg(test)]` blocks

Update tests to compile and pass with the new field names. Key changes:

1. **ledger.rs tests**: `folder: Some(...)` -> `domain: Some(...)` in LedgerEntry construction
2. **markdown.rs tests**: Remove `assert!(rendered.contains("day:"))` and `assert!(rendered.contains("time:"))`. Add assertions for `domain:` and `origin: assisted`. Change `uploader:` -> `creator:`, `duration_min:` -> `duration:`, `trace_id:` -> `trace:`, `author:` -> `creator:`. Add `domain` field to all `NoteContent` construction. Update `FrontmatterConfig` references (`default_author` -> `default_creator`).
3. **pipeline.rs tests**: Update `resolve_destination` tests (simplified signature, always returns `notes/`). Update `resolve_vocab_folder` -> `resolve_vocab_domain` tests. Remove emoji paths from test data.
4. **router.rs tests**: `folder:` -> `domain:` in LinkConfig and UrlMatch construction. Update `format_reply` assertions ("Folder:" -> "Domain:").
5. **telegram.rs tests**: `folder:` -> `domain:` in IngestResult construction. Update URL expectations (no more `Inbox%2F`).
6. **assets.rs tests**: Update `"⚙️ System/attachments"` assertions to `"system/attachments"`.
7. **fabric.rs tests**: Update `"folder"` JSON keys and `.folder` assertions to `"domain"` / `.domain`.
8. **config.rs tests**: Update `inbox_path` assertion from `"/tmp/vault/Inbox"` to `"/tmp/vault/inbox"`.

### CLAUDE.md Update

After implementation, update the project CLAUDE.md to reflect:
- New frontmatter schema (replace the current schema block)
- New architecture diagram (folder references -> domain references)
- Update path references (`⚙️ System` -> `system`)

## Alternatives Considered

### Alternative 1: Config-Driven Path Mapping

- **Description:** Add a `[paths]` section to config that maps logical names to filesystem paths, keeping the folder routing logic intact
- **Pros:** Minimal code change, just update config values
- **Cons:** Preserves the wrong abstraction (folder-based routing). The vault's model is property-driven - the code should match
- **Why not chosen:** We'd still need the frontmatter changes and the semantic shift. Better to do it cleanly once

### Alternative 2: Gradual Migration with Feature Flag

- **Description:** Add a `vault_version: v2` config flag that switches between old and new behavior
- **Pros:** Can run both modes during transition
- **Cons:** Doubles the code paths, increases maintenance burden, transition is already done
- **Why not chosen:** The vault migration is complete. There is no v1 to support

### Alternative 3: Domain as Subfolder Under notes/

- **Description:** Route notes to `notes/ai/`, `notes/tech/`, `notes/football/` instead of flat `notes/`
- **Pros:** Some filesystem organization preserved
- **Cons:** Contradicts the vault design decision. The whole point of the migration was that organization is property-driven, not folder-driven. Obsidian's search and Dataview work on properties, not paths
- **Why not chosen:** Goes against the architectural decision already made and implemented in the vault

## Technical Considerations

### Dependencies

- No new crate dependencies
- Fabric `obsidian_classify` pattern may need updating separately (returns `folder` key in JSON). The `#[serde(alias = "folder")]` bridge handles this until the pattern is updated

### Performance

No performance impact. This is a rename/simplify - no new computation, no new I/O.

### Security

No security implications. Path changes are from emoji to lowercase - no new filesystem access patterns.

### Testing Strategy

1. `cargo test` after each phase - fix broken assertions incrementally
2. Manual smoke test: ingest one YouTube URL and one article URL after all phases
3. Verify rendered frontmatter matches `system/frontmatter.md` schema
4. Verify ledger entry uses Domain column
5. Verify note lands in `notes/` directory

### Rollout Plan

1. Implement all four phases in obsidian-borg repo
2. Update live config file
3. Run `cargo test`
4. Manual smoke test with a real URL
5. Commit and resume obsidian-borg service

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Fabric classify pattern returns `folder` not `domain` | High | Low | `#[serde(alias = "folder")]` handles both |
| Existing ledger has Folder column, new code writes Domain | Med | Low | The vault ledger was already migrated to Domain column. `check_duplicate` parses by column position (cols[4]=Status, cols[6]=Source), not header name. Column positions are unchanged. Update the comment on ledger.rs:94 |
| YAML config field renames break deserialization | Med | Med | Use `#[serde(alias = "...")]` for backward compat on config fields |
| Tests reference hardcoded emoji strings | High | Low | Update all test assertions in Phase 4 |
| `build_obsidian_url` generates wrong deep links | Med | Med | Post-migration all notes are in `notes/`, so Obsidian URLs change from `file=Inbox%2Fnote.md` to `file=notes%2Fnote.md`. Update tests and verify deep links work in Obsidian |
| Old ledger rows have Folder column values, new rows have Domain | Low | Low | Both old ("Folder") and new ("Domain") rows are at the same column position. `check_duplicate` reads by position, not by header. No functional impact |
| `migrate.rs` skip-folders references old folder names | Low | Low | Migration is complete. The skip-folders config was already updated. If migrate is re-run, it reads from config, not hardcoded values |

## Open Questions

- [ ] Should `research_date_subfolder` be removed entirely or kept as a no-op? (Recommendation: remove - no notes go to subfolder paths anymore)
- [ ] Should the `obsidian_classify` Fabric pattern be updated to return `domain` instead of `folder`? (Recommendation: yes, but after this work, using serde alias as bridge)
- [ ] Should `inbox_path` be removed from VaultConfig entirely? (Recommendation: keep for now - it defines where `inbox/` is, even though `resolve_destination` no longer uses it. Other code paths may need it in the future for triage workflows)

## References

- `~/repos/scottidler/obsidian/system/frontmatter.md` - canonical frontmatter schema
- `~/repos/scottidler/obsidian/system/domain-values.md` - allowed domain values
- `~/repos/scottidler/obsidian/system/origin-values.md` - allowed origin values
- `~/repos/scottidler/obsidian/system/type-values.md` - allowed type values
- `~/repos/scottidler/obsidian/CLAUDE.md` - vault architecture and conventions
- `docs/design/2026-03-07-canonicalization-dedup-dashboard.md` - ledger and dedup design
- `docs/design/2026-03-17-content-type-expansion-and-quality-gates.md` - recent content type additions
