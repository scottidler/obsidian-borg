# Design Document: Fabric Pattern Templating from Vault Schema

**Author:** Scott Idler
**Date:** 2026-03-17
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Fabric patterns used by obsidian-borg (notably `obsidian_classify`) contain hardcoded lists of domain values, type values, and other enums that duplicate the vault's canonical schema files. When the schema evolves, the pattern files drift out of sync - as just happened when vault v2 domains were not reflected in the classify pattern. This design adds a `sync-patterns` subcommand that reads vault schema files and templates them into Fabric pattern files, making the vault the single source of truth.

## Problem Statement

### Background

obsidian-borg uses Fabric patterns for LLM-powered content processing. The `obsidian_classify` pattern contains a hardcoded list of allowed domain values that the LLM must pick from. The vault maintains canonical schema reference files (`system/domain-values.md`, `system/type-values.md`, etc.) that define these enums.

On 2026-03-17, an ingest produced `domain: "🤖 Tech/ai-llm"` instead of `domain: "ai"` because the Fabric pattern still listed the old emoji folder paths from vault v1. The vault had been migrated to v2 domain values, but nobody updated the Fabric pattern. A `normalize_domain()` safety net was added after the fact, but the root cause remains: two sources of truth for the same data.

### Problem

1. The `obsidian_classify` pattern duplicates domain values from `system/domain-values.md`
2. Future patterns (e.g., a type classifier, tag validator) will duplicate other schema enums
3. When schema evolves, pattern files must be manually updated - easy to forget
4. The LLM sees stale instructions and produces invalid values
5. The `normalize_domain()` safety net catches known legacy values but cannot anticipate every possible LLM hallucination from a stale prompt

### Goals

- Vault schema files (`system/domain-values.md`, `system/type-values.md`, etc.) are the single source of truth for all enum values
- Fabric patterns are generated/updated automatically from these schema files
- Schema changes require zero manual edits to Fabric patterns
- The solution works for all current and future custom patterns (`obsidian_classify`, `obsidian_note`, and any new ones)
- Existing non-custom patterns (e.g., `extract_article_wisdom`, `create_tags`) are never modified

### Non-Goals

- Modifying upstream Fabric patterns (only obsidian-borg's custom patterns)
- Auto-detecting when schema files change (the user runs the command)
- Generating entirely new patterns from scratch (only templating enum values into existing pattern templates)
- Moving Fabric patterns into the obsidian-borg repo (they stay in `~/.config/fabric/patterns/`)
- Generating the `normalize_domain()` alias table from schema (follow-up work - see Open Questions)

## Proposed Solution

### Overview

1. Pattern **templates** live in `~/.config/obsidian-borg/patterns/` (configurable via `patterns-dir`)
2. Templates use a simple placeholder syntax: `{{domain-values}}`, `{{type-values}}`, etc.
3. A new `obsidian-borg sync-patterns` subcommand reads vault schema files, parses the value tables, and renders templates into `~/.config/fabric/patterns/`
4. The daemon startup optionally runs sync-patterns automatically (config flag)

### Data Flow

```
vault/system/domain-values.md  --.
vault/system/type-values.md   ---+--> schema parser --> placeholder map
vault/system/origin-values.md --'          |
                                           v
~/.config/obsidian-borg/patterns/*.tmpl --> template engine --> ~/.config/fabric/patterns/*/system.md
```

### Template Format

Templates are plain markdown files identical to Fabric pattern `system.md` files, but with placeholders where enum values appear.

Example `patterns/obsidian_classify/system.md.tmpl`:

```markdown
# IDENTITY and PURPOSE

You are an expert content classifier for an Obsidian vault. Given a title and summary
of content, you classify it into the most appropriate domain.

# DOMAINS

The allowed domain values are:

{{domain-values}}

# OUTPUT

Return ONLY a JSON object with no markdown formatting:

{
  "domain": "The best matching domain from the list above",
  "confidence": 0.0 to 1.0 confidence score,
  "reasoning": "Brief explanation of classification",
  "suggested_tags": ["tag1", "tag2", "tag3"]
}

# RULES

- Pick the MOST SPECIFIC domain that matches
- If content spans multiple domains, pick the dominant one
- If unsure, set confidence below 0.6
- Do not invent domains not in the list above
- Values must be exactly as shown: single lowercase word, no hyphens, no emojis, no paths
- Do not output anything except the JSON object

# INPUT

INPUT:
```

### Schema Parser

Each vault schema file follows a consistent structure: a `## Values` section containing a markdown table with `| Value | Description | ... |` columns. The parser finds the first markdown table after the `## Values` heading and extracts value-description pairs from the first two columns. The header row and separator row are skipped. Additional columns (e.g., "Replaces folder") are ignored.

For `domain-values.md`, the table:

```
| ai | AI, LLMs, agents, prompting, AI tools | Tech/ai-llm |
| tech | Programming, CLI tools, DevOps, ... | Tech/tools, ... |
```

Gets rendered as:

```
- "ai" -- AI, LLMs, agents, prompting, AI tools
- "tech" -- Programming, CLI tools, DevOps, ...
```

The third column ("Replaces folder") is ignored - it's migration context, not pattern context.

### Placeholder Registry

| Placeholder | Source File | Columns Used |
|-------------|------------|--------------|
| `{{domain-values}}` | `system/domain-values.md` | Value, Description |
| `{{type-values}}` | `system/type-values.md` | Value, Description |
| `{{origin-values}}` | `system/origin-values.md` | Value, Description |

New placeholders can be added by extending the registry in config or code.

### Data Model

```rust
/// A single enum value parsed from a vault schema file
#[derive(Debug, Clone)]
pub struct SchemaValue {
    pub value: String,
    pub description: String,
}

/// A parsed schema file
#[derive(Debug, Clone)]
pub struct SchemaEnum {
    pub name: String,           // e.g., "domain-values"
    pub values: Vec<SchemaValue>,
}

/// A pattern template to render
#[derive(Debug, Clone)]
pub struct PatternTemplate {
    pub name: String,           // e.g., "obsidian_classify"
    pub template_path: PathBuf, // ~/.config/obsidian-borg/patterns/obsidian_classify/system.md.tmpl
    pub output_path: PathBuf,   // ~/.config/fabric/patterns/obsidian_classify/system.md
}
```

### CLI Interface

```
obsidian-borg sync-patterns [--dry-run] [--pattern <name>]
```

- `--dry-run`: Show what would be written without writing
- `--pattern <name>`: Only sync a specific pattern (default: all templates in `patterns/`)

Output:

```
Reading vault schema: system/domain-values.md (10 values)
Reading vault schema: system/type-values.md (13 values)
Rendering: obsidian_classify/system.md (1 placeholder replaced)
Synced 1 pattern.
```

### Implementation Plan

#### Phase 1: Schema Parser Module

New file: `src/schema.rs`

1. `parse_schema_file(path: &Path) -> Result<SchemaEnum>` - reads a vault schema markdown file, extracts the first table, parses Value and Description columns
2. `load_schemas(vault_root: &Path) -> Result<HashMap<String, SchemaEnum>>` - loads all known schema files into a map keyed by placeholder name

#### Phase 2: Template Engine

New file: `src/patterns.rs`

1. `find_templates(template_dir: &Path) -> Result<Vec<PatternTemplate>>` - discovers all `.tmpl` files under the configured template directory
2. `render_template(template: &str, schemas: &HashMap<String, SchemaEnum>) -> Result<String>` - replaces `{{placeholder}}` tokens with rendered schema values. Unresolved placeholders (no matching schema) produce a warning and are left as-is so the pattern remains functional but visibly incomplete.
3. `format_schema_list(schema: &SchemaEnum) -> String` - formats as `- "value" -- description` lines. Rendered output is prefixed with a comment: `<!-- Generated by obsidian-borg sync-patterns. Do not edit - modify the .tmpl file instead. -->`
4. `write_pattern(output_path: &Path, content: &str) -> Result<()>` - writes to a temp file in the same directory, then renames atomically to avoid partial writes during active Fabric calls

#### Phase 3: CLI Subcommand

In `src/cli.rs`:

1. Add `SyncPatterns` variant to the CLI enum with `--dry-run` and `--pattern` flags
2. In `src/main.rs`: wire up the subcommand to call `patterns::sync_all()` or `patterns::sync_one()`

#### Phase 4: Optional Daemon Startup Sync

In `src/config.rs`:

1. Add `sync_patterns_on_start: bool` to config (default: `false`)
2. In daemon startup path, call `patterns::sync_all()` if enabled - log warnings on failure but do not block startup
3. Default is `false` because the vault may not be accessible at daemon start (e.g., network mount). Users opt in after verifying their setup.

#### Phase 5: Create Template Files

1. Create `~/.config/obsidian-borg/patterns/obsidian_classify/system.md.tmpl` with `{{domain-values}}` placeholder based on the current pattern
2. Add `patterns-dir` config key pointing to the templates directory (default: `~/.config/obsidian-borg/patterns/`)
3. Only template patterns that actually use placeholders - do not copy patterns that have no dynamic content

#### Phase 6: Tests

1. Schema parser tests: parse a sample markdown table, verify extracted values
2. Template rendering tests: verify placeholder replacement, verify unknown placeholders are left alone (with warning)
3. End-to-end test: template + schema -> rendered output matches expected

## Alternatives Considered

### Alternative 1: Embed Schema Values in Rust Code

- **Description:** Define domain values as a Rust enum or const array, generate the Fabric pattern at build time via `build.rs` or a proc macro
- **Pros:** Compile-time guarantee that code and patterns are in sync
- **Cons:** The vault schema files are the source of truth, not the Rust code. This would create a third source of truth (vault files, Rust enum, rendered pattern). Also requires recompilation for schema changes.
- **Why not chosen:** Moves authority away from the vault. The vault schema files are designed to be human-editable and referenced by Obsidian Dataview queries. They must remain authoritative.

### Alternative 2: Read Schema at Classify Time

- **Description:** Instead of templating patterns, have `classify_topic()` read domain-values.md at runtime and inject the values into the Fabric prompt dynamically (via stdin prefix or system prompt injection)
- **Pros:** Always up to date, no sync step needed
- **Cons:** Fabric patterns are files on disk - the `-p` flag loads a pattern by name from `~/.config/fabric/patterns/`. There is no way to dynamically inject into a pattern without either modifying the file or bypassing Fabric's pattern system entirely. Bypassing would mean reimplementing Fabric's prompt assembly.
- **Why not chosen:** Would require abandoning Fabric's pattern system or writing a custom LLM caller - too much complexity for the gain.

### Alternative 3: Git Hook in Vault Repo

- **Description:** Add a post-commit hook to the obsidian vault repo that regenerates Fabric patterns when schema files change
- **Pros:** Fully automatic, no manual sync step
- **Cons:** Couples the vault repo to obsidian-borg's pattern generation. The vault is a content repo - it should not have build tooling dependencies. Also, the hook would need access to obsidian-borg's template files.
- **Why not chosen:** Wrong separation of concerns. obsidian-borg should own its own pattern generation.

### Alternative 4: Jinja2 / Tera Templates with External Renderer

- **Description:** Use a full template engine (Tera for Rust, or shell out to Jinja2) for richer template syntax
- **Pros:** Supports conditionals, loops, filters - future-proof
- **Cons:** Overkill for simple placeholder replacement. Adds a dependency (Tera) or external tool (Jinja2/Python). The templates only need `{{name}}` -> list substitution.
- **Why not chosen:** YAGNI. Simple string replacement covers all current and foreseeable needs. Can upgrade to Tera later if templates get complex.

## Technical Considerations

### Dependencies

- No new crate dependencies. The schema parser and template engine are simple string processing.
- The vault must be accessible at the configured `vault.root-path` when sync-patterns runs.

### Performance

Negligible. Parsing a few small markdown files and writing a few pattern files takes milliseconds.

### Security

No security implications. Pattern files are local, read from a trusted vault, written to a user-owned config directory.

### Testing Strategy

1. Unit tests for schema parser (handles edge cases: empty tables, missing columns, extra whitespace)
2. Unit tests for template rendering (placeholder replacement, no-match passthrough, multiple placeholders)
3. Integration test: render `obsidian_classify` template with test schema data, verify output matches expected pattern

### Rollout Plan

1. Implement schema parser and template engine
2. Create template files in `patterns/`
3. Wire up CLI subcommand
4. Run `sync-patterns --dry-run` to verify output
5. Run `sync-patterns` to write patterns
6. Verify next ingest produces correct domain values
7. Optionally enable `sync-patterns-on-start` in config

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Schema file format changes (table structure) | Low | Med | Parser validates expected columns, fails loudly if format is unexpected |
| Template placeholder typo | Low | Low | Log warning for unresolved placeholders, do not silently produce broken patterns |
| Vault not mounted/accessible when sync runs | Low | Med | Graceful failure with clear error message. Daemon startup sync is best-effort. |
| Fabric pattern directory does not exist | Low | Low | Create directory if missing |
| sync-patterns overwrites manual edits to pattern files | Med | Low | Templates are the source, not the rendered files. Document this clearly. `--dry-run` lets users preview. |
| Partial write during active classify call | Low | Low | Write to a temp file and rename atomically. Standard practice for config-file generators. |
| Schema file has broken/missing table | Low | Med | Parser returns error with file path and line number. sync-patterns aborts for that schema but continues others. |

## Open Questions

- [ ] Should `sync-patterns` also validate that rendered domain values match what `normalize_domain()` expects, as a consistency check?
- [ ] Should the `obsidian_note` pattern also use templated values (e.g., for output format instructions), or is it stable enough to leave as-is?
- [ ] Should template files use `.tmpl` extension or just be `.md` files in a `patterns/` directory?
- [ ] Should `normalize_domain()`'s `VALID_DOMAINS` and `DOMAIN_ALIASES` also be generated from vault schema files? Currently they are a separate hardcoded list that must be kept in sync manually. Could be a follow-up to this work.

## References

- `~/repos/scottidler/obsidian/system/domain-values.md` - canonical domain enum
- `~/repos/scottidler/obsidian/system/type-values.md` - canonical type enum
- `~/repos/scottidler/obsidian/system/origin-values.md` - canonical origin enum
- `~/repos/scottidler/obsidian/system/frontmatter.md` - canonical frontmatter schema
- `~/.config/fabric/patterns/obsidian_classify/system.md` - current (manually maintained) classify pattern
- `~/.config/fabric/patterns/obsidian_note/system.md` - current note rendering pattern
- `docs/design/2026-03-17-vault-v2-alignment.md` - vault v2 migration that exposed this problem
