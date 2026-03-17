# Design Document: Content Type Expansion, Fetch Quality Gates, and Ledger Audit

**Author:** Scott Idler
**Date:** 2026-03-17
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Expand obsidian-borg's content classification beyond `youtube` and the catch-all `article` to include `github`, `social`, and `reddit` types. Add a content quality gate that detects blocked/garbage content (Cloudflare challenges, CAPTCHAs, empty pages) and fails the ingest rather than creating junk notes. Introduce an `audit` subcommand that scans the borg ledger and vault notes to find misclassified entries and optionally re-ingest or fix them.

## Problem Statement

### Background

The URL classification system in `router.rs` uses a config-driven `links` list with regex patterns. Currently only three patterns exist: `shorts`, `youtube`, and `default` (catch-all `.*`). Any URL that isn't YouTube falls through to `default`, and `pipeline.rs` treats all non-YouTube URLs as articles (`ContentType::Article`). The content is fetched via a three-tier fallback: `fabric -u` -> `markitdown-cli` -> Jina Reader.

### Problem

1. **Missing content types.** GitHub repo URLs (e.g., `github.com/open-webui/open-terminal`) are classified as `type: article`. They should be `type: github`. Similarly, X/Twitter posts and Reddit threads are classified as articles when they are distinct content types with different summarization needs.

2. **Blocked content creates junk notes.** When a website serves a Cloudflare challenge page, CAPTCHA, or paywall interstitial, the pipeline happily saves that garbage as a note. The result is notes with titles like "Just a moment..." and bodies containing anti-bot HTML. The pipeline has no validation that the fetched content is actually the requested content.

3. **Raw URL titles.** When content fetch partially fails (title extraction fails but content succeeds), notes get their raw URL as the title (e.g., `https://github.com/NousResearch/hermes-agent`). This is a symptom of the same missing quality gate.

4. **No way to audit historical data.** The borg ledger has ~100 entries. Several are known misclassified (GitHub repos marked as articles, blocked content marked as successful). There's no tooling to scan and identify these retroactively.

### Observed Misclassifications

From the current vault and ledger:

**GitHub repos typed as `article` (7 notes):**
- `github.com/open-webui/open-terminal`
- `github.com/czl9707/build-your-own-openclaw`
- `github.com/Infatoshi/OpenSquirrel`
- `github.com/NousResearch/hermes-agent` (2 copies)
- `github.com/ForLoopCodes/contextplus`
- `github.com/okta-awscli/okta-awscli`

**Social media posts typed as `article` (2 notes):**
- `x.com/Zai_org/status/...` (X/Twitter post)
- `reddit.com/r/footballstrategy/...` (Reddit thread)

**Blocked/garbage content saved as successful (1 note):**
- `blog.adafruit.com/...` - title is "Just a moment..." (Cloudflare block)

**Raw URL as title (8+ notes):**
- `https://github.com/NousResearch/hermes-agent`
- `https://github.com/ForLoopCodes/contextplus`
- `https://sendfame.com/ai-image-generator`
- `https://www.snowflake.com/en/redefining-data-engineering-in-the-age-of-ai/`
- `https://venturebeat.com/orchestration/...`
- `https://www.infoq.com/news/2026/03/...`
- And more

### Goals

- Add `github` content type for GitHub repository URLs (`github.com/<owner>/<repo>`)
- Add `social` content type for X/Twitter posts (`x.com/<user>/status/<id>`)
- Add `reddit` content type for Reddit threads (`reddit.com/r/<sub>/comments/<id>/...`)
- Detect blocked/garbage content after fetch and fail the ingest with a clear error
- Detect raw-URL-as-title and treat it as a fetch quality failure
- Add `obsidian-borg audit` subcommand to scan ledger + vault for misclassified entries
- Extend `migrate` to handle `type` reclassification for the new content types
- Keep everything config-driven and extensible

### Non-Goals

- GitHub API integration (authentication, rate limits, structured metadata) - just use the existing fetch pipeline with better classification
- Full social media API clients (Twitter API, Reddit API)
- Automatic re-ingestion of failed/blocked URLs - audit identifies them, user decides
- Custom summarization patterns per content type (future enhancement)
- URL shortener resolution (`t.co`, `bit.ly`)

## Proposed Solution

### Overview

Three changes, each independent but complementary:

```
Change 1: Content Type Expansion    new link patterns + ContentType variants
Change 2: Fetch Quality Gate        validate content after fetch, before note creation
Change 3: Ledger Audit              scan + report misclassified entries
```

### Change 1: Content Type Expansion

#### New Link Patterns

Add to `default_links()` in `config.rs`, ordered before the catch-all `default`:

```rust
fn default_links() -> Vec<LinkConfig> {
    vec![
        // ... existing shorts, youtube ...
        LinkConfig {
            name: "github".to_string(),
            regex: r"https?://github\.com/[^/]+/[^/]+/?(\?[^ ]*)?$".to_string(),
            resolution: "FWVGA".to_string(),
            folder: "".to_string(),
        },
        LinkConfig {
            name: "social".to_string(),
            regex: r"https?://x\.com/[^/]+/status/\d+".to_string(),
            resolution: "FWVGA".to_string(),
            folder: "".to_string(),
        },
        LinkConfig {
            name: "reddit".to_string(),
            regex: r"https?://(?:www\.)?reddit\.com/r/[^/]+/comments/".to_string(),
            resolution: "FWVGA".to_string(),
            folder: "".to_string(),
        },
        // ... existing default catch-all ...
    ]
}
```

The `github` regex uses `$` anchoring to match only `github.com/<owner>/<repo>` (with optional trailing slash and query params) but NOT `github.com/<owner>/<repo>/blob/...` (code links), `github.com/<owner>/<repo>/issues/...` (issues), or `github.com/blog/...` (GitHub's own blog). Those deeper paths fall through to the `default` catch-all and are treated as articles. Since `classify_url()` uses `regex.is_match()`, the `$` anchor is critical to prevent the github pattern from matching URLs with additional path segments.

**Important:** The regexes run in order, first match wins. The catch-all `default` must remain last. The new patterns must come after `youtube`/`shorts` but before `default`.

#### New ContentType Variants

Add to the `ContentType` enum in `markdown.rs`:

```rust
pub enum ContentType {
    YouTube { uploader: String, duration_secs: f64 },
    Article,
    GitHub,       // NEW
    Social,       // NEW
    Reddit,       // NEW
    Image { asset_path: String },
    Pdf { asset_path: String },
    Audio { asset_path: String, duration_secs: Option<f64> },
    Note,
    VocabDefine { word: String, language: String },
    VocabClarify { word_a: String, word_b: String, language: String },
    Document { asset_path: String },
    Code { language: String },
}
```

And the `type_field` match arm in `render_note()`:

```rust
ContentType::GitHub => "github",
ContentType::Social => "social",
ContentType::Reddit => "reddit",
```

#### Pipeline Routing

Update `process_url_inner()` in `pipeline.rs` to branch on the new types. Initially, all three new types use the same article fetch pipeline (fabric -> markitdown -> Jina). The key difference is the `ContentType` variant written to frontmatter:

```rust
let (title, summary, content_type) = if url_match.is_youtube_type() {
    // ... existing youtube handling ...
} else {
    let content_type_for = |name: &str| match name {
        "github" => ContentType::GitHub,
        "social" => ContentType::Social,
        "reddit" => ContentType::Reddit,
        _ => ContentType::Article,
    };
    let ct = content_type_for(&url_match.link_name);
    if use_fabric {
        match process_article_fabric(&url_match.url, config).await {
            Ok((title, summary, _)) => (title, summary, ct),
            Err(e) => {
                log::warn!("Fabric article fetch failed: {e:#}, falling back to Jina");
                let (title, summary, _) = process_article_jina(&url_match.url).await?;
                (title, summary, ct)
            }
        }
    } else {
        let (title, summary, _) = process_article_jina(&url_match.url).await?;
        (title, summary, ct)
    }
};
```

The `process_article_fabric` and `process_article_jina` functions continue to return `ContentType::Article` internally, but the caller overrides it with the correct type based on `link_name`. This avoids duplicating the fetch logic.

#### Migration Config Update

Add the new types to the `reclassify` transform in `migrate.rs`. The existing `reclassify` transform converts `type: link` to `youtube` or `article` based on the source URL. Extend it to also produce `github`, `social`, and `reddit`:

```rust
"reclassify" => {
    if let Some(source) = fields.get("source") {
        let source = source.trim_matches('"');
        if youtube_re.is_match(source) || shorts_re.is_match(source) {
            "youtube".to_string()
        } else if github_repo_re.is_match(source) {
            "github".to_string()
        } else if x_status_re.is_match(source) {
            "social".to_string()
        } else if reddit_re.is_match(source) {
            "reddit".to_string()
        } else {
            "article".to_string()
        }
    }
}
```

This means `obsidian-borg migrate --apply` will fix existing misclassified notes.

### Change 2: Fetch Quality Gate

#### Problem

The pipeline fetches content and blindly trusts whatever comes back. Cloudflare challenge pages, CAPTCHAs, paywall interstitials, and empty responses all get saved as notes.

#### Detection Strategy

After content is fetched (by any method: fabric, markitdown, Jina) and before note creation, run a quality check. Two layers:

**Layer 1: Heuristic checks (fast, no LLM cost)**

Check the fetched content for known garbage patterns:

```rust
/// Known block page title patterns (high confidence - these are almost never real titles)
const BLOCKED_TITLE_INDICATORS: &[&str] = &[
    "just a moment",
    "attention required",
    "access denied",
    "one more step",
    "please verify you are a human",
];

/// Known block page content patterns (require short content to trigger)
const BLOCKED_CONTENT_INDICATORS: &[&str] = &[
    "checking your browser",
    "enable javascript and cookies",
    "ray id:",
    "cf-browser-verification",
    "please turn javascript on",
    "captcha",
    "sucuri website firewall",
    "ddos protection by",
];

fn detect_blocked_content(content: &str, title: &str) -> Option<String> {
    let lower_content = content.to_lowercase();
    let lower_title = title.to_lowercase();

    // Check title for known block page titles (high confidence)
    for indicator in BLOCKED_TITLE_INDICATORS {
        if lower_title.contains(indicator) {
            return Some(format!("Blocked content detected in title: '{title}'"));
        }
    }

    // Check if content is suspiciously short (< 500 chars after trimming)
    // combined with block indicators in the body
    let trimmed = content.trim();
    if trimmed.len() < 500 {
        for indicator in BLOCKED_CONTENT_INDICATORS {
            if lower_content.contains(indicator) {
                return Some(format!("Blocked content detected: short content with '{indicator}'"));
            }
        }
    }

    // Check if title is a raw URL (fetch failed to extract a real title)
    // This is a warning - the content may still be usable, but the title needs attention
    if lower_title.starts_with("http://") || lower_title.starts_with("https://") {
        return Some(format!("Title is a raw URL, content fetch likely failed: '{title}'"));
    }

    None
}
```

**Note on false positives:** The `BLOCKED_CONTENT_INDICATORS` list deliberately omits generic words like "cloudflare" that could appear in legitimate articles about Cloudflare products. The content indicators only trigger when combined with short content (< 500 chars), which makes false positives very unlikely - a real article about Cloudflare would have substantial content.

**Layer 2: LLM validation (optional, for borderline cases)**

Not implemented in this phase. The heuristic layer catches the known patterns. If we find false negatives in practice, we can add an LLM check later.

#### Integration Point

In `process_url_inner()`, after content fetch and title extraction, before tag generation and note writing:

```rust
let (title, summary, content_type) = /* ... fetch content ... */;

// Quality gate: detect blocked/garbage content
if let Some(reason) = detect_blocked_content(&summary, &title) {
    bail!("Content quality check failed: {reason}");
}
```

This causes the ingest to fail with a clear error message. The failure is logged to the ledger as `FailedReason::BlockedContent` (or just included in the generic `Failed { reason }` string). The URL can be retried later.

#### New Error Variant

No new error type needed. The existing `IngestStatus::Failed { reason: String }` is sufficient. The `reason` string will clearly indicate the blocked content detection:

```
Failed (2.3s): Content quality check failed: Blocked content detected in title: 'Just a moment...'
```

This shows up in Telegram/Discord replies and the ledger, making it obvious what happened.

### Change 3: Ledger Audit

#### New Subcommand

Add `obsidian-borg audit` that scans the ledger and vault for problems:

```
obsidian-borg audit                  # report only
obsidian-borg audit --fix            # fix misclassified types via migrate
obsidian-borg audit --re-ingest      # re-ingest blocked/failed entries
```

#### Audit Checks

1. **Type misclassification:** For each `status: Completed` ledger entry, check if the source URL would classify differently under the current link patterns. Report entries where the existing note's `type:` frontmatter doesn't match what the current router would produce.

2. **Blocked content detection:** For each `status: Completed` ledger entry, read the corresponding vault note and run `detect_blocked_content()` against its body and title. Report notes that would now fail the quality gate.

3. **Raw URL titles:** For each `status: Completed` ledger entry, check if the title starts with `http://` or `https://`. These are notes where title extraction failed.

4. **Duplicate notes:** Check for multiple notes with the same canonical source URL (e.g., the two `hermes-agent` notes in different folders).

#### Output Format

```
$ obsidian-borg audit

Audit Results:
  7 misclassified types (article -> github)
  2 misclassified types (article -> social/reddit)
  1 blocked content saved as completed
  8 raw URL titles
  1 duplicate note pair

Details:
  [MISTYPE] github.com/open-webui/open-terminal -> type should be: github (currently: article)
  [MISTYPE] github.com/Infatoshi/OpenSquirrel -> type should be: github (currently: article)
  ...
  [BLOCKED] blog.adafruit.com/... -> title: "Just a moment..."
  [RAW-TITLE] github.com/NousResearch/hermes-agent -> title is raw URL
  ...
  [DUPLICATE] github.com/NousResearch/hermes-agent -> 2 notes found
```

#### Fix Behavior

`--fix` runs the equivalent of `migrate --apply` but only on notes identified by the audit. It updates `type:` frontmatter to match current classification rules.

`--re-ingest` marks identified broken entries (blocked content, raw URL titles) in the ledger as `status: FailedAudit` and re-queues them for processing. The re-ingest respects the normal pipeline (including the new quality gate), so if the site is still blocking, it will fail again cleanly.

### Data Model Changes

#### ContentType enum (markdown.rs)

Add three variants: `GitHub`, `Social`, `Reddit`. Each maps to a string in frontmatter (`github`, `social`, `reddit`).

#### LinkConfig defaults (config.rs)

Add three entries before the catch-all. Users can override or extend via config file.

#### No schema migration needed

The frontmatter schema doesn't change - `type:` field already exists, it just gets new valid values. The `migrate` reclassify transform handles updating existing notes.

### Implementation Plan

**Phase 1: Content Type Expansion**
- Add `GitHub`, `Social`, `Reddit` variants to `ContentType`
- Add link patterns to `default_links()`
- Update pipeline routing to use correct `ContentType` based on `link_name`
- Update `render_note()` type field mapping
- Update `migrate.rs` reclassify transform
- Add tests for new URL classification
- Run `migrate --dry-run` to verify existing notes would be fixed

**Phase 2: Fetch Quality Gate**
- Implement `detect_blocked_content()` in a new `quality.rs` module (or in `hygiene.rs`)
- Wire into `process_url_inner()` after content fetch
- Add tests with known blocked content patterns
- Test with the known `blog.adafruit.com` blocked content case

**Phase 3: Ledger Audit**
- Add `audit` subcommand to CLI
- Implement ledger scanning and cross-referencing with vault notes
- Implement `--fix` for type corrections
- Implement `--re-ingest` for broken entries

## Alternatives Considered

### Alternative 1: GitHub API for Repo Metadata

- **Description:** Use GitHub's REST API to fetch structured repo data (description, stars, language, topics) instead of treating repos as articles.
- **Pros:** Rich, structured metadata; better titles; repo-specific frontmatter fields
- **Cons:** Requires authentication for rate limits; adds API dependency; over-engineered for the current need
- **Why not chosen:** The immediate problem is classification, not summarization. The existing fetch pipeline works fine for GitHub README content. Can add API integration later as an enhancement.

### Alternative 2: LLM-Based Content Type Classification

- **Description:** Instead of regex patterns, send the URL to an LLM to classify the content type.
- **Pros:** More flexible; handles edge cases; could detect types we haven't thought of
- **Cons:** Slow; expensive; non-deterministic; the URL structure is deterministic enough for regex
- **Why not chosen:** URL domain/path patterns are highly reliable for type classification. `github.com/<owner>/<repo>` is always a GitHub repo. LLM classification is overkill here.

### Alternative 3: Extend the Existing `reclassify` Transform Only

- **Description:** Don't add new `ContentType` variants - just fix the migration reclassify to produce `github`/`social`/`reddit` strings without changing the enum.
- **Pros:** Minimal code change
- **Cons:** New notes would still be created with `type: article`; only migration would produce the correct types; the enum wouldn't reflect reality
- **Why not chosen:** This only fixes historical data, not the ongoing bug. New GitHub URLs would still be classified as articles.

### Alternative 4: Content Validation via LLM

- **Description:** Send fetched content to an LLM to ask "is this real content or a block page?"
- **Pros:** Catches novel block patterns; very accurate
- **Cons:** Adds latency and cost to every ingest; the known patterns are well-defined
- **Why not chosen:** Heuristic detection catches 95%+ of cases. Can add LLM validation as a fallback later if needed.

## Technical Considerations

### Dependencies

No new external dependencies. All changes use existing crates (regex, eyre, serde).

### Performance

- New link patterns add 3 regex matches per URL (fast, microseconds)
- Quality gate is a string scan of already-fetched content (no additional I/O)
- Audit subcommand reads the ledger and vault notes (I/O bound, but one-time operation)

### Security

No security implications. No new network calls, no new authentication.

### Testing Strategy

- Unit tests for new URL classification patterns (github, social, reddit)
- Unit tests for `detect_blocked_content()` with known garbage content
- Unit tests for edge cases (github.com/blog vs github.com/owner/repo)
- Integration test: ingest a URL that would be classified as github
- Manual test: run `audit` against current vault to verify detection

### Rollout Plan

1. Implement Phase 1, run `cargo test`, deploy
2. Run `obsidian-borg migrate --dry-run` to preview type fixes
3. Run `obsidian-borg migrate --apply` to fix existing notes
4. Implement Phase 2, deploy - new ingests now have quality gate
5. Implement Phase 3, run `audit` to find remaining issues

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| GitHub regex matches non-repo URLs (e.g., github.com/settings) | Low | Low | Regex anchors to `/<owner>/<repo>` pattern with exactly 2 path segments |
| Quality gate false positives (real content triggers blocked detection) | Low | Med | Heuristics are conservative; only trigger on short content + known indicators; can tune thresholds |
| Quality gate false negatives (novel block patterns slip through) | Med | Low | Notes still get created (same as today); can add patterns as discovered |
| Audit --fix modifies wrong notes | Low | High | Always dry-run first; audit is read-only by default; --fix uses same migrate machinery |
| New content types break Dataview queries in dashboard | Low | Low | Dashboard queries use `type:` field but don't filter by specific values |

## Open Questions

- [ ] Should `github.com/<owner>/<repo>/issues/<num>` be `github` or a separate `github-issue` type?
- [ ] Should `reddit` include all reddit URLs or just threads? (What about user profiles, subreddit pages?)
- [ ] Should the quality gate be configurable (enable/disable, custom patterns) or always-on?
- [ ] For `audit --re-ingest`, should we delete the old broken note or leave it and create a new one?

## References

- Design doc: `docs/design/2026-03-07-canonicalization-dedup-dashboard.md` - original dedup/ledger design
- Current router: `src/router.rs` - URL classification
- Current pipeline: `src/pipeline.rs` - content fetch and note creation
- Current migration: `src/migrate.rs` - reclassify transform
- Borg ledger: `~/repos/scottidler/obsidian/⚙️ System/borg-ledger.md`
