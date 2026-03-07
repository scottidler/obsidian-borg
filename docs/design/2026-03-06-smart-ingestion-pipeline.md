# Design Document: Smart Ingestion Pipeline

**Author:** Scott Idler
**Date:** 2026-03-06
**Status:** In Review
**Review Passes Completed:** 5/5 + Fabric-first rewrite

## Summary

Transform obsidian-borg from a "dump everything in Inbox" system into a smart ingestion pipeline that leverages Fabric as the primary workhorse for content fetching, summarization, tagging, and classification. Fabric already handles YouTube transcripts (`-y`), URL scraping (`-u` via Jina), multi-model LLM routing, and has 234 battle-tested patterns. We lean on it heavily instead of reimplementing these capabilities in Rust. obsidian-borg becomes the orchestrator: URL hygiene, config-driven routing, Fabric invocation, note rendering, and vault writing.

## Problem Statement

### Background

obsidian-borg currently ingests URLs from Telegram, Discord, and HTTP POST, processes them (YouTube transcription or Jina Reader for articles), and writes a markdown note to a single `Inbox` folder. The sibling project obsidian-bookmark has mature tech for URL classification, rich frontmatter, YouTube API metadata, and config-driven folder routing that obsidian-borg lacks.

The Obsidian vault at `~/repos/scottidler/obsidian/` has a well-established folder taxonomy:

```
📥 Inbox/              -- current dump target
🤖 Tech/              -- AI-LLM, Rust, NixOS, Python, Tools, research/
🏈 Football/          -- research/
✍️ Writing/           -- Craft, Projects
💼 Work/              -- Infrastructure
📚 Resources/         -- Articles, Videos, Books, People
🧠 Knowledge/         -- English-Vocab, Health, Learning
🎵 Music/
🇪🇸 Spanish/          -- Vocabulary
📆 Daily/             -- date-organized journals
⚙️ System/            -- Agents, Archive, Attachments, Templates
```

**Note:** All folder references in config use the actual emoji-prefixed names as they exist on disk.

### Problem

1. **Everything lands in Inbox.** Notes require manual triage to reach their destination. This defeats the "second brain" principle that classification should be automatic.
2. **Minimal metadata.** Notes have bare frontmatter (title, date, source, type, tags). No author, published date, day/time, embed codes, or configurable defaults.
3. **No URL hygiene.** UTM parameters and tracking cruft pass through untouched. No tag sanitization.
4. **No summarization.** YouTube notes contain raw transcripts. Articles contain raw Jina markdown. No LLM-generated summaries.
5. **Hardcoded URL classification.** `url_router.rs` uses a fixed `is_youtube_host()` match. Can't add new URL types without code changes.
6. **Reimplements what Fabric already does.** Our `youtube.rs` shells out to yt-dlp for transcripts, `jina.rs` calls Jina Reader, `transcription_client.rs` manages Groq/Whisper — Fabric has all of this built in (`-y`, `-u`, `--transcribe-file`).

### Goals

- Fabric-first architecture: delegate content fetching, summarization, tagging, and classification to Fabric
- Config-driven URL classification via regex patterns, each mapping to a name, folder, and resolution
- UTM parameter stripping and URL cleanup on all incoming URLs
- Tag sanitization (lowercase, special chars to hyphens)
- Rich frontmatter with configurable defaults and merge logic
- YouTube iframe embed codes with configurable resolution
- LLM-powered topic classification to route content to the correct vault folder
- Confidence-based routing: low confidence falls back to Inbox
- Date-based subfolders for research content
- Custom Fabric patterns for vault-specific classification

### Non-Goals

- Browser extension (capture is via Telegram/Discord/HTTP only)
- Web scraping with `scraper` crate (Fabric + Jina + markitdown handle this)
- Building a custom LLM client (Fabric handles model routing)
- Changing the vault folder taxonomy (use what exists)
- Real-time sync or Obsidian plugin integration
- `pip install` anything (use `pipx` exclusively)
- Replacing Fabric's REST API with our own LLM proxy

## Proposed Solution

### Overview — Fabric as the Workhorse

The key insight: **Fabric already does most of what we were planning to build**:

| Capability | Before (custom Rust) | After (Fabric) |
|---|---|---|
| YouTube transcripts | `youtube.rs` + yt-dlp shell-out | `fabric -y <url> --transcript` |
| YouTube metadata | `youtube.rs` + yt-dlp `--dump-json` | `fabric -y <url> --metadata` |
| Article-to-markdown | `jina.rs` HTTP call to `r.jina.ai` | `fabric -u <url>` (uses Jina internally) |
| Audio transcription | `transcription_client.rs` + Groq | `fabric --transcribe-file` |
| Summarization | Not implemented | `fabric -p create_summary` / `youtube_summary` / `extract_article_wisdom` |
| Tag generation | Not implemented | `fabric -p create_tags` |
| Content rating | Not implemented | `fabric -p label_and_rate` (returns JSON) |
| Topic classification | Not implemented | Custom `obsidian_classify` pattern (returns JSON) |
| Multi-model routing | Config but unused | Fabric's `-m` flag + `FABRIC_MODEL_PATTERN_NAME` env vars |
| Output to file | `pipeline.rs` write | `fabric -o <path>` (or we render ourselves for richer control) |

obsidian-borg's role becomes **orchestrator**: URL hygiene, config-driven regex routing, Fabric invocation, note assembly, and vault writing.

### Pipeline

```
URL arrives (Telegram/Discord/HTTP)
    |
    v
[1. URL Hygiene] ─── strip UTM, normalize (Rust)
    |
    v
[2. URL Classification] ─── config regex match (Rust)
    |
    v
[3. Content Fetch] ─── fabric -y <url> (YouTube) or fabric -u <url> (article)
    |                    fallback: markitdown <url>, then jina.rs
    v
[4. Summarize] ─── fabric -p youtube_summary (YT) or extract_article_wisdom (article)
    |
    v
[5. Generate Tags] ─── fabric -p create_tags
    |
    v
[6. Classify Topic] ─── fabric -p obsidian_classify (custom pattern, returns JSON)
    |
    v
[7. Route] ─── Tier 1: URL-type folder from config (if non-empty, done)
    |           Tier 2: LLM-classified folder (if confidence >= threshold)
    |           Tier 3: fallback to 📥 Inbox
    v
[8. Render Note] ─── rich frontmatter + embed + summary + body (Rust)
    |
    v
[9. Write to Vault] ─── target folder, date subfolder if research (Rust)
```

### Architecture

#### Fabric Integration (`fabric.rs` — new module, replaces `jina.rs` and most of `youtube.rs`)

The core interface to Fabric. All calls shell out to the `fabric` binary:

```rust
/// Run any Fabric pattern against input text
pub async fn run_pattern(pattern: &str, input: &str, config: &FabricConfig) -> Result<String>

/// Fetch YouTube content: transcript + metadata
pub async fn fetch_youtube(url: &str, config: &FabricConfig) -> Result<YouTubeContent> {
    // fabric -y <url> --transcript -> transcript text
    // fabric -y <url> --metadata -> JSON metadata (title, channel, duration, etc.)
}

/// Fetch article content as markdown
pub async fn fetch_article(url: &str, config: &FabricConfig) -> Result<String> {
    // fabric -u <url> -> markdown
    // fallback: shell out to markitdown <url>
    // fallback: jina.rs HTTP call (kept as last resort)
}

/// Summarize content using configured pattern
pub async fn summarize(content: &str, is_youtube: bool, config: &FabricConfig) -> Result<String> {
    // YouTube: fabric -p youtube_summary
    // Article: fabric -p extract_article_wisdom
    // Configurable via fabric.summarize_pattern_youtube / fabric.summarize_pattern_article
}

/// Generate tags from content
pub async fn generate_tags(content: &str, config: &FabricConfig) -> Result<Vec<String>> {
    // fabric -p create_tags -> "tag1 tag2 tag3"
    // parse space-separated output into Vec<String>
}

/// Classify content into a vault folder (custom pattern)
pub async fn classify_topic(
    title: &str,
    summary: &str,
    config: &FabricConfig,
    routes: &[TopicRoute],
) -> Result<ClassificationResult> {
    // fabric -p obsidian_classify
    // returns JSON: { "folder": "🤖 Tech/AI-LLM", "confidence": 0.85, "tags": ["ai", "llm"] }
}

pub struct YouTubeContent {
    pub title: String,
    pub channel: String,
    pub duration_secs: f64,
    pub published_at: String,
    pub transcript: String,
    pub video_id: String,
}

pub struct ClassificationResult {
    pub folder: String,
    pub confidence: f64,
    pub tags: Vec<String>,
}
```

Key Fabric flags used:

| Flag | Purpose |
|---|---|
| `-y <url>` | Fetch YouTube transcript (uses yt-dlp internally) |
| `-y <url> --metadata` | Fetch YouTube metadata as JSON |
| `-y <url> --transcript-with-timestamps` | Transcript with timestamps |
| `-u <url>` | Scrape URL to markdown via Jina AI |
| `-p <pattern>` | Run a specific pattern |
| `-m <model>` | Choose model (e.g., `Anthropic\|claude-sonnet-4-5`) |
| `-o <file>` | Write output to file |
| `--search` | Enable web search for supported models |

#### Custom Fabric Pattern: `obsidian_classify`

Installed at `~/.config/fabric/patterns/obsidian_classify/system.md`:

```markdown
# IDENTITY and PURPOSE

You are an expert content classifier for an Obsidian vault. Given a title and summary
of content, you classify it into the most appropriate vault folder.

# VAULT FOLDERS

The available destination folders are:

- "🤖 Tech/AI-LLM" — AI, LLMs, machine learning, Claude, GPT, Anthropic, OpenAI
- "🤖 Tech/Rust" — Rust programming, cargo, crates
- "🤖 Tech/NixOS" — NixOS, Nix, flakes
- "🤖 Tech/Python" — Python programming, pip, Django, Flask
- "🤖 Tech/Tools" — Developer tools, CLI tools, productivity software
- "🏈 Football/research" — Football plays, offense, defense, coaching, drills
- "✍️ Writing/Craft" — Writing craft, fiction, novels, storytelling
- "💼 Work" — Work-related, infrastructure, SRE, platform engineering
- "📚 Resources/Articles" — General articles not fitting other categories
- "📚 Resources/Videos" — General videos not fitting other categories
- "🧠 Knowledge/Health" — Health, fitness, wellness
- "🧠 Knowledge/Learning" — Learning techniques, education
- "🎵 Music" — Music, instruments, songs
- "🇪🇸 Spanish" — Spanish language learning

# OUTPUT

Return ONLY a JSON object with no markdown formatting:

{
  "folder": "The best matching folder path from the list above",
  "confidence": 0.0 to 1.0 confidence score,
  "reasoning": "Brief explanation of classification",
  "suggested_tags": ["tag1", "tag2", "tag3"]
}

# RULES

- Pick the MOST SPECIFIC folder that matches
- If content spans multiple categories, pick the dominant one
- If unsure, set confidence below 0.6
- Do not invent folders not in the list above
- Do not output anything except the JSON object

# INPUT

INPUT:
```

This pattern is config-driven — the vault folder list can be regenerated from `routing.routes` config.

#### Modules Simplified/Removed

| Module | Change |
|---|---|
| `jina.rs` | **Kept as fallback only.** Fabric `-u` handles primary article fetching. `jina.rs` becomes a thin fallback if Fabric is unavailable. |
| `youtube.rs` | **Drastically simplified.** Remove `fetch_subtitles()`, `extract_audio()`, `clean_vtt()`. Keep `extract_video_id()` and `generate_embed_code()`. Fabric `-y` handles all content fetching. `fetch_metadata()` via yt-dlp kept as fallback. |
| `transcription_client.rs` | **Removed.** Fabric `--transcribe-file` and `-y` handle transcription. Groq fallback is no longer our concern. |
| `markdown.rs` | **Rewritten.** Rich frontmatter, embed codes, summary section. |
| `pipeline.rs` | **Rewritten.** Orchestrates Fabric calls instead of doing the work itself. |
| `fabric.rs` | **New.** Core Fabric integration module. |
| `url_hygiene.rs` | **New.** UTM stripping, tag sanitization, filename sanitization. |

#### Config Changes (`config.rs` + YAML)

```yaml
vault:
  root_path: "~/repos/scottidler/obsidian/"

links:
  - name: shorts
    regex: 'https?://(?:www\.)?youtube\.com/shorts/([a-zA-Z0-9_-]+)'
    resolution: 480p
    folder: "📚 Resources/Videos"
  - name: youtube
    regex: 'https?://(?:www\.)?(youtube\.com/watch\?v=|youtu\.be/)([a-zA-Z0-9_-]+)'
    resolution: FWVGA
    folder: "📚 Resources/Videos"
  - name: default
    regex: '.*'
    resolution: FWVGA
    folder: ""  # empty = use LLM classification

frontmatter:
  defaults:
    tags: []
    author: ""
    published: ""
  timezone: "America/Los_Angeles"

fabric:
  binary: "fabric"  # assumes on PATH
  model: ""  # empty = use fabric's default model (currently claude-sonnet-4)
  summarize_pattern_youtube: "youtube_summary"
  summarize_pattern_article: "extract_article_wisdom"
  tag_pattern: "create_tags"
  classify_pattern: "obsidian_classify"
  max_content_chars: 30000  # truncate before sending to LLM

routing:
  confidence_threshold: 0.6
  fallback_folder: "📥 Inbox"
  research_date_subfolder: true
  routes:
    - keywords: ["ai", "llm", "machine learning", "claude", "gpt", "openai", "anthropic"]
      folder: "🤖 Tech/AI-LLM"
    - keywords: ["rust", "cargo", "crate"]
      folder: "🤖 Tech/Rust"
    - keywords: ["nix", "nixos", "flake"]
      folder: "🤖 Tech/NixOS"
    - keywords: ["python", "pip", "django", "flask"]
      folder: "🤖 Tech/Python"
    - keywords: ["football", "offense", "defense", "play", "drill", "coaching"]
      folder: "🏈 Football/research"
    - keywords: ["writing", "fiction", "novel", "story", "craft"]
      folder: "✍️ Writing/Craft"
    - keywords: ["spanish", "espanol", "vocabulary"]
      folder: "🇪🇸 Spanish"
    - keywords: ["music", "guitar", "piano", "song"]
      folder: "🎵 Music"
```

**Note:** `vault.inbox_path` is kept as a deprecated alias for backward compatibility — if present, it is used as `routing.fallback_folder` when `routing.fallback_folder` is not set. All folders in config are relative to `vault.root_path`.

The `youtube` section (with `api_key_env`) is removed — Fabric's `-y --metadata` provides YouTube metadata. If we need the YouTube API v3 for richer data (tags, description), we add it back as an optional enrichment step.

#### URL Hygiene Module (`url_hygiene.rs`)

```rust
pub fn clean_url(raw: &str) -> Result<String>  // strip UTM + all tracking params, normalize
pub fn sanitize_tag(tag: &str) -> String        // lowercase, special -> hyphens
pub fn sanitize_filename(title: &str) -> String // safe for filesystem
```

Ported from obsidian-bookmark's `remove_utm_source()`, `sanitize_tag()`, `sanitize_filename()`.

#### URL Classification (`url_router.rs` rewrite)

```rust
pub struct LinkConfig {
    pub name: String,
    pub regex: String,
    pub resolution: String,
    pub folder: String,
}

pub struct UrlMatch {
    pub url: String,
    pub link_name: String,       // "youtube", "shorts", "default", or custom
    pub folder: String,          // from config, may be empty (= use LLM routing)
    pub width: usize,
    pub height: usize,
}

pub fn classify_url(raw: &str, links: &[LinkConfig]) -> Result<UrlMatch>
```

Pipeline uses `link_name` to choose Fabric invocation strategy:
- `"youtube"` or `"shorts"` -> `fabric -y <url>`
- anything else -> `fabric -u <url>` (fallback: `markitdown`, then `jina.rs`)

#### Rich Frontmatter (`markdown.rs` rewrite)

```rust
pub struct Frontmatter {
    pub title: String,
    pub date: String,
    pub day: String,
    pub time: String,
    pub tags: Vec<String>,
    pub url: String,
    pub author: String,
    pub published: String,
    pub content_type: String,  // youtube, article, shorts
    pub uploader: Option<String>,
    pub duration_min: Option<u32>,
}

impl Frontmatter {
    pub fn merge(defaults: &FrontmatterDefaults, actual: &Frontmatter) -> Frontmatter
}

pub fn render_note(note: &NoteContent) -> String

pub struct NoteContent {
    pub frontmatter: Frontmatter,
    pub embed_code: Option<String>,  // YouTube iframe
    pub summary: String,             // Fabric-generated
    pub body: String,                // full transcript or article markdown
}
```

#### Pipeline (`pipeline.rs` rewrite)

```rust
async fn process_url_inner(url: &str, tags: Vec<String>, config: &Config) -> Result<IngestResult> {
    // 1. Clean URL
    let url = url_hygiene::clean_url(url)?;

    // 2. Classify URL type via config regex
    let url_match = url_router::classify_url(&url, &config.links)?;

    // 3. Fetch content via Fabric
    let (content, metadata) = match url_match.link_name.as_str() {
        name if is_youtube_type(name) => {
            let yt = fabric::fetch_youtube(&url, &config.fabric).await?;
            (yt.transcript, ContentMeta::YouTube(yt))
        }
        _ => {
            let article = fabric::fetch_article(&url, &config.fabric).await?;
            (article, ContentMeta::Article)
        }
    };

    // 4. Summarize via Fabric pattern
    let is_youtube = matches!(metadata, ContentMeta::YouTube(_));
    let summary = fabric::summarize(&content, is_youtube, &config.fabric).await
        .unwrap_or_default();  // graceful: no summary is OK

    // 5. Generate tags via Fabric
    let mut all_tags = fabric::generate_tags(&content, &config.fabric).await
        .unwrap_or_default();
    all_tags.extend(tags);
    all_tags = all_tags.into_iter().map(|t| url_hygiene::sanitize_tag(&t)).collect();

    // 6. Resolve destination folder
    let folder = if !url_match.folder.is_empty() {
        // Tier 1: URL-type routing from config
        url_match.folder.clone()
    } else {
        // Tier 2: LLM topic classification
        match fabric::classify_topic(&title, &summary, &config.fabric, &config.routing.routes).await {
            Ok(result) if result.confidence >= config.routing.confidence_threshold => {
                all_tags.extend(result.tags);
                result.folder
            }
            _ => config.routing.fallback_folder.clone(),  // Tier 3: Inbox
        }
    };

    // 7. Build and render note
    let note = build_note(metadata, &url, &summary, &content, &all_tags, &url_match, config);
    let rendered = markdown::render_note(&note);

    // 8. Write to vault
    let dest = resolve_path(&config.vault.root_path, &folder, &config.routing);
    let filename = url_hygiene::sanitize_filename(&note.frontmatter.title);
    let note_path = write_note(&dest, &filename, &rendered, &url)?;

    Ok(IngestResult { ... })
}
```

### Data Model

#### Resolution Maps (from obsidian-bookmark)

```rust
const RESOLUTIONS: &[(&str, (usize, usize))] = &[
    ("nHD", (640, 360)), ("FWVGA", (854, 480)), ("SD", (1280, 720)),
    ("FHD", (1920, 1080)), ("4K", (3840, 2160)),
];

const SHORTS_RESOLUTIONS: &[(&str, (usize, usize))] = &[
    ("480p", (480, 854)), ("720p", (720, 1280)), ("1080p", (1080, 1920)),
];
```

### API Design

No changes to the HTTP API (`POST /ingest`, `GET /health`). Telegram and Discord bot interfaces remain the same. All changes are internal pipeline improvements.

`IngestResult` gains a field:

```rust
pub struct IngestResult {
    pub status: IngestStatus,
    pub note_path: Option<String>,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub elapsed_secs: Option<f64>,
    pub folder: Option<String>,  // NEW: where the note was routed
}
```

### Implementation Plan

**Phase 1: URL Hygiene & Config-Driven Classification**
- Add `url_hygiene.rs` with `clean_url()`, `sanitize_tag()`, improved `sanitize_filename()`
- Add `links` config section with regex patterns
- Rewrite `url_router.rs` to use config-driven regex matching
- Add resolution maps
- Wire `clean_url()` into pipeline entry point

**Phase 2: Fabric Integration (the big one)**
- Add `fabric.rs` module with `fetch_youtube()`, `fetch_article()`, `summarize()`, `generate_tags()`, `run_pattern()`
- Add `fabric` config section
- Rewrite `pipeline.rs` to use Fabric for content fetching and summarization
- Simplify `youtube.rs` to just `extract_video_id()` + `generate_embed_code()`
- Keep `jina.rs` as fallback only
- Remove `transcription_client.rs`

**Phase 3: Rich Frontmatter & Note Rendering**
- Rewrite `Frontmatter` struct with all fields
- Add `frontmatter.defaults` config section with merge logic
- Timezone-aware date/day/time generation (add `chrono-tz`)
- YouTube iframe embed codes with configurable resolution
- Update `render_note()` for rich output

**Phase 4: Smart Routing & Classification**
- Create custom `obsidian_classify` Fabric pattern
- Add `routing` config section with routes and confidence threshold
- Implement `classify_topic()` in `fabric.rs`
- Two-tier routing: URL-type folder -> LLM classification -> fallback
- Date-based subfolder creation for research content
- Backward compatibility: `vault.inbox_path` as deprecated alias

## Alternatives Considered

### Alternative 1: Keep custom Rust implementations for everything
- **Description:** Don't use Fabric. Keep building yt-dlp wrappers, Jina client, Groq transcription client, and add our own LLM API calls.
- **Pros:** No external binary dependency, everything in Rust
- **Cons:** Reimplements what Fabric already does. More code to maintain. No access to 234 patterns. Must manage API keys and model routing ourselves. Must handle yt-dlp edge cases ourselves.
- **Why not chosen:** Fabric is installed, configured, battle-tested, and actively maintained. It handles YouTube transcripts, Jina scraping, multi-model LLM routing, and per-pattern model mapping. Building all this in Rust is wasted effort.

### Alternative 2: Use Fabric REST API instead of CLI shell-out
- **Description:** Run `fabric --serve` and call its REST API from Rust
- **Pros:** Avoid process spawn overhead, structured JSON responses
- **Cons:** Requires running a persistent Fabric server. Additional process to manage. More complex deployment.
- **Why not chosen:** Shell-out is simpler and sufficient for our throughput (one URL at a time from bots). Can revisit if performance becomes an issue.

### Alternative 3: Web scraping with `scraper` crate for articles
- **Description:** Parse HTML directly to extract metadata
- **Pros:** No external dependency
- **Cons:** Fragile, maintenance burden, Fabric + Jina + markitdown already do this better
- **Why not chosen:** Fabric `-u` uses Jina internally. `markitdown` handles PDFs and Office docs. No need for our own scraper.

### Alternative 4: Always route to Inbox, classify later
- **Description:** Keep current behavior, add a separate classification pass
- **Pros:** Simpler pipeline
- **Cons:** Defeats the goal. Per "Build a Second Brain 2026" research: "The number one reason second brains fail is they require taxonomy work at capture time."
- **Why not chosen:** Inline classification is the core value proposition.

## Technical Considerations

### Dependencies

**Existing Rust crates (no changes):**
- `reqwest`, `regex`, `serde`/`serde_yaml`/`serde_json`, `chrono`, `url`

**Existing Rust crates (reduced usage):**
- `yt-dlp` — only for `extract_video_id` regex and fallback metadata

**New Rust deps:**
- `chrono-tz` — timezone-aware date formatting

**External tools (all already installed):**
- `fabric` — Go binary at `/home/saidler/go/bin/fabric`. Default model: `claude-sonnet-4`. Patterns symlinked from `~/repos/danielmiessler/fabric/data/patterns/`. Config at `~/.config/fabric/`.
- `markitdown` — already installed, article-to-markdown fallback
- `yt-dlp` — still used by Fabric internally for YouTube

**Removed Rust deps (potentially):**
- `teloxide`, `serenity` — kept (Telegram/Discord bots are still ours)
- Groq/Whisper integration — removed, Fabric handles transcription

### Performance

- Fabric shell-out: ~2-5s per pattern invocation
- Pipeline runs 3-4 Fabric calls sequentially: fetch (~5-15s) + summarize (~3-5s) + tags (~2-3s) + classify (~2-3s)
- Total: ~12-26s per URL (vs current ~5-30s for transcription-dominated pipeline)
- Acceptable for async bot workflow. Can parallelize summarize + tags calls.

### Security

- Fabric inherits API keys from its own config (`~/.config/fabric/.env`)
- No API keys in obsidian-borg config (YouTube API key removed since Fabric handles metadata)
- UTM stripping improves privacy of stored URLs
- Custom patterns are local files, not shared

### Testing Strategy

- Unit tests for `url_hygiene.rs`: UTM stripping, tag sanitization, filename sanitization
- Unit tests for `url_router.rs`: regex matching against config patterns
- Unit tests for `markdown.rs`: frontmatter rendering, merge logic, embed code generation
- Integration test for `fabric.rs`: verify binary exists, verify patterns are available via `fabric -l`
- Integration test: end-to-end with a known YouTube URL and article URL
- Existing tests for routes, types, health remain valid

### Rollout Plan

Each phase is independently deployable. Phase 1 (URL hygiene) is pure improvement. Phase 2 (Fabric integration) is the biggest change but simplifies the codebase. Phase 4 (smart routing) adds the classification layer with confidence-gated fallback to Inbox.

### Graceful Degradation

The system must always produce a note, even when external tools fail:

| Component | If unavailable or fails | Fallback |
|---|---|---|
| `fabric` binary | No content fetch, no summary, no classification | Fall back to current pipeline (yt-dlp + jina.rs); route to `📥 Inbox` |
| `fabric -y` (YouTube) | Can't fetch transcript | Fall back to `youtube.rs` yt-dlp direct call |
| `fabric -u` (article) | Can't scrape URL | Fall back to `markitdown <url>`, then `jina.rs` |
| `fabric -p create_tags` | Tag generation fails | Use empty tags (or tags from YouTube metadata) |
| `fabric -p obsidian_classify` | Classification fails | Route to `routing.fallback_folder` (`📥 Inbox`) |
| Topic classification | Low confidence | Route to `📥 Inbox` |

**Principle:** a note with less metadata in the right-ish place is always better than no note at all.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Fabric binary not on PATH | Medium | High | Check at startup, log warning, keep current pipeline as fallback |
| Fabric pattern doesn't exist | Low | High | Verify via `fabric -l` at startup; skip missing patterns gracefully |
| Fabric CLI interface changes | Low | High | Pin Fabric version; wrap calls with output validation |
| Shell-out overhead too slow | Low | Medium | Can parallelize independent Fabric calls; can switch to REST API (`--serve`) |
| LLM classification picks wrong folder | Medium | Low | Confidence threshold; low confidence -> Inbox |
| Fabric model costs (API calls per note) | Medium | Low | Configure cheap model for tags/classify; expensive model for summarize only |
| Config migration for existing users | Low | Low | `#[serde(default)]` on all new fields; `inbox_path` kept as deprecated alias |
| Duplicate filenames | Medium | Medium | Append short hash of source URL if file exists |
| Long transcripts exceed context window | Medium | Medium | `fabric.max_content_chars` truncation before LLM calls |
| `markitdown` not installed | Low | Medium | Fall back to Fabric `-u` / Jina Reader |

## Open Questions

- [x] ~~Should we use Fabric or build custom LLM integration?~~ **Fabric. It already does everything.**
- [x] ~~What Fabric pattern for classification?~~ **Custom `obsidian_classify` pattern returning JSON.**
- [ ] Should `routing.routes` keywords be used as a fast pre-filter before LLM classification (Rust-side keyword match skips the Fabric call)?
- [ ] Do we want a "fix" mechanism where Telegram/Discord users can correct a misclassification?
- [ ] Should research content always get date subfolders, or only when classified as "research" type?
- [ ] Should Fabric calls for tags and classification run in parallel?
- [ ] Per-pattern model mapping: should we use cheap models (e.g., `claude-haiku-4-5`) for tags/classify and expensive models for summarization?
- [ ] How to handle URLs that produce no usable content (404, paywall, empty page)?

## References

- obsidian-bookmark source: `~/repos/scottidler/obsidian-bookmark/src/main.rs`
- obsidian-bookmark config: `~/repos/scottidler/obsidian-bookmark/obsidian-bookmark.yml`
- Fabric README: `~/repos/danielmiessler/fabric/README.md`
- Fabric patterns: `~/repos/danielmiessler/fabric/data/patterns/`
- Fabric `label_and_rate` pattern: returns JSON with `one-sentence-summary`, `labels`, `rating`, `quality-score`
- Fabric `create_tags` pattern: returns space-separated lowercase tags
- Fabric `youtube_summary` pattern: structured markdown with timestamps
- Fabric `extract_article_wisdom` pattern: SUMMARY, IDEAS, QUOTES, FACTS, REFERENCES, RECOMMENDATIONS
- Fabric key flags: `-y` (YouTube), `-u` (URL scrape), `-p` (pattern), `-m` (model), `-o` (output file), `--metadata`
- Fabric per-pattern model mapping: `FABRIC_MODEL_PATTERN_NAME=vendor|model` env vars
- "Build a Second Brain 2026": `~/repos/scottidler/obsidian/🤖 Tech/research/2026-03-01/build-a-second-brain-2026.md`
- "Build a Second Brain 2026 pt2": `~/repos/scottidler/obsidian/🤖 Tech/research/2026-03-01/build-a-second-brain-2026-pt2.md`
- "Obsidian with Gemini": `~/repos/scottidler/obsidian/🤖 Tech/research/2026-03-01/obsidian-with-gemini.md`
- "Complete System Wish I Knew Sooner": `~/repos/scottidler/obsidian/🤖 Tech/research/2026-03-01/complete-system-wish-i-knew-sooner.md`
