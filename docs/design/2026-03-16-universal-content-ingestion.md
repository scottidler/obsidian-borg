# Design Document: Universal Content Ingestion

**Author:** Scott Idler
**Date:** 2026-03-16
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Expand obsidian-borg from a URL-only ingestion pipeline to a universal content ingestion engine. The system will accept URLs, images, PDFs, plain text, audio/voice memos, documents, and code snippets - classifying, processing, and routing each to the appropriate vault location. This transforms obsidian-borg from a bookmark manager into the central intake funnel for a true second brain.

## Problem Statement

### Background

obsidian-borg currently ingests URLs (articles and YouTube videos) from six input sources (Telegram, Discord, HTTP, clipboard hotkey, CLI, ntfy) and writes summarized Obsidian notes to the vault. The pipeline is URL-centric: `extract_url_from_text()` looks for URLs in messages, `classify_url()` routes by URL pattern, and `process_url()` fetches remote content.

The vault already contains non-URL content that was created manually or pasted in without proper processing:
- **PDFs** sitting loose in `🏈 Football/` with ad-hoc companion notes
- **Pasted images** in `⚙️ System/attachments/` with timestamp names, opaque to search
- **Manual notes** across many domains (vocab definitions, coaching plays, book outlines, checklists, work summaries)
- **Templates** for content types that obsidian-borg doesn't know about (`vocab.md`, `book.md`, `note.md`)

### Problem

1. **URL-only pipeline.** Sending anything that isn't a URL results in "No URL found in message." The Telegram bot, Discord bot, HTTP endpoint, and CLI all reject non-URL input.

2. **Images are opaque.** Photos of whiteboards, screenshots, diagrams - these get pasted into Obsidian manually with no OCR, no tags, no searchability. A football play diagram is invisible to vault search.

3. **PDFs are unprocessed.** PDF files end up in the vault without text extraction or summary notes. The tools exist (`markitdown-cli`, Fabric) but the pipeline doesn't handle file inputs.

4. **Quick thoughts have no path.** "Met James at the Rust meetup" or "4-2-5 blitz from weak side" - there's no way to fire these into the vault from Telegram/Discord/CLI and have them properly classified and routed.

5. **Audio/voice notes are unsupported.** The Whisper transcription infrastructure exists (built for YouTube) but there's no path for voice memos from Telegram or audio files from CLI.

6. **No structured note detection.** The vault has conventions for vocab notes (definitions, word comparisons) but obsidian-borg can't detect triggers like `define: <word>` or `clarify: <word> vs <word>` and produce properly structured notes.

### Goals

- Generalize the pipeline from `process_url()` to `process_content()` with a `ContentKind` discriminator
- Accept images via all input sources, run OCR/vision, store originals in attachments, create searchable notes
- Accept PDFs and documents, extract text via `markitdown-cli`, summarize via Fabric, store originals
- Accept plain text as quick capture, classify intent via LLM, route to appropriate vault folder
- Accept audio/voice memos, transcribe via Whisper, create notes from transcription
- Detect structured patterns in text input: `define: <word>` for vocab definitions, `clarify: <word> vs <word>` for vocab comparisons (both work for English and Spanish)
- Accept code snippets with language detection
- Maintain backward compatibility - URL ingestion works exactly as before
- All new content types integrate with existing dedup, ledger, routing, and tagging infrastructure

### Non-Goals

- Book clippings / Kindle integration - not a current reading modality
- Email forwarding ingestion - may revisit later
- Browser extension / bookmarklet changes - existing browser capture is sufficient
- Podcast RSS auto-ingestion - separate concern
- Clipboard watcher mode - existing hotkey is sufficient
- Daily digest generation - handled by a separate process
- Changing how humans manually create notes in the vault - we only formalize ingestion
- Contacts/people as a first-class type - can be handled by plain text routing to `🧠 Knowledge/people/`

## Proposed Solution

### Overview

Replace the URL-centric pipeline with a content-type-aware pipeline. Input sources detect what kind of content they received (URL, image, file, text, audio) and pass it to a unified `process_content()` function that dispatches to type-specific handlers. All handlers converge on the existing `render_note()` and routing infrastructure.

```
Input (any source)
    |
    v
[Input source determines ContentKind from message type / file extension / MIME]
    |
    +-- ContentKind::Url       -> process_url()          [existing pipeline, unchanged]
    +-- ContentKind::Image     -> process_image()        [store + OCR/vision -> note]
    +-- ContentKind::Pdf       -> process_pdf()          [store + markitdown -> summarize -> note]
    +-- ContentKind::Audio     -> process_audio()        [store + transcribe -> note]
    +-- ContentKind::Text      -> process_text()         [pattern detect -> LLM classify -> note]
    |                              |
    |                              +-- "define: word"    -> vocab definition note
    |                              +-- "clarify: a vs b" -> vocab comparison note
    |                              +-- code detected     -> code snippet note
    |                              +-- other             -> general note (LLM routes)
    +-- ContentKind::Document  -> process_document()     [store + markitdown -> summarize -> note]
    |
    v
render_note()                  # Frontmatter + body (extended for new types)
    |
    v
route + write + ledger         # Existing infrastructure
```

### Architecture

#### Content Classification

Classification happens at two levels:

**Level 1: Input sources** (Telegram, Discord, HTTP, CLI) determine content kind based on what they received. This is not an LLM call - it's simple dispatch based on message type:

| Source | Signal | ContentKind |
|--------|--------|-------------|
| Telegram `message.photo()` | Photo attachment | `Image` |
| Telegram `message.document()` | MIME type `application/pdf` | `Pdf` |
| Telegram `message.document()` | MIME type `image/*` | `Image` |
| Telegram `message.document()` | MIME type `audio/*` | `Audio` |
| Telegram `message.document()` | MIME type `application/vnd.*`, `.docx`, `.epub` | `Document` |
| Telegram `message.voice()` | Voice note | `Audio` |
| Telegram `message.text()` | Contains URL | `Url` |
| Telegram `message.text()` | No URL | `Text` |
| CLI `--file <path>` | File extension | `Image`, `Pdf`, `Audio`, or `Document` |
| CLI `<url>` | URL argument | `Url` |
| CLI `note "..."` | Text argument | `Text` |

**Level 2: `process_text()` sub-classification** determines intent within plain text (this uses pattern matching first, then LLM):

1. If text matches `define: <word>` -> vocab definition note (language auto-detected from the word)
2. If text matches `clarify: <word> vs <word>` -> vocab comparison note (language auto-detected)
3. If text looks like code (heuristic: indentation, brackets, keywords) -> code snippet
4. Otherwise -> LLM classifies intent + topic for routing

```rust
pub enum ContentKind {
    Url(String),                                           // URL string
    Image { data: Vec<u8>, filename: String },             // Raw image bytes
    Pdf { data: Vec<u8>, filename: String },               // PDF bytes
    Audio { data: Vec<u8>, filename: String },             // Audio bytes (ogg, mp3, wav)
    Text(String),                                          // Plain text / quick capture
    Document { data: Vec<u8>, filename: String },          // docx, pptx, epub, etc.
}
```

Note: `Code` is not a separate `ContentKind` - it's detected during `process_text()` sub-classification. The input source can't distinguish code from text; only content analysis can.

#### Input Source Changes

**Telegram** (`telegram.rs`):
- Currently only handles `message.text()` and extracts URLs
- Extend to handle:
  - `message.photo()` - Telegram sends photos as PhotoSize arrays; download the largest
  - `message.document()` - file attachments (PDF, docx, etc.) with MIME type
  - `message.voice()` - OGG Opus voice notes
  - `message.audio()` - audio files
  - Text without URLs - pass as plain text capture
- Telegram's `getFile` API downloads file bytes given a `file_id`
- **Note:** Telegram Bot API limits file downloads to 20MB. Files larger than this must be sent via CLI or HTTP.

**Discord** (`discord.rs`):
- Extend to handle message attachments (Discord `Attachment` struct has `content_type`, `url`, `filename`)
- Voice messages appear as attachments with `audio/ogg` content type
- Text without URLs - pass as plain text capture

**HTTP endpoint** (`routes.rs`):
- New endpoint: `POST /ingest` with `multipart/form-data` support (in addition to existing JSON)
- Multipart fields: `file` (binary), `text` (string), `tags` (string), `force` (bool)
- Existing JSON body still works for URL ingestion (backward compatible)
- Alternative: new `POST /ingest/file` endpoint to keep concerns separate

**CLI** (`cli.rs`):
- `obsidian-borg ingest <url>` - unchanged
- `obsidian-borg ingest --file <path>` - ingest a local file (image, PDF, audio, document)
- `obsidian-borg note "met James at the Rust meetup"` - new subcommand for quick text capture
- `obsidian-borg note --clipboard` - capture clipboard text (not URL) as a note

#### Processing Pipelines

**process_image()**:
```
image bytes
  -> store_asset(attachments/images/YYYY-MM/)
  -> OCR via tesseract (fast, local, text extraction)
  -> Vision LLM via Claude API (description, context, tag suggestions)
  -> Combine: OCR text + LLM description
  -> Generate title from LLM description
  -> Generate tags from content
  -> render_note() with ![[image-embed]] and extracted text
  -> Route by content topic
```

Tesseract provides raw text extraction (great for screenshots, documents, whiteboards with text). The Claude vision API provides semantic understanding (what is this image of, what's the context). Both are useful - tesseract for searchable text, vision LLM for intelligent tagging and routing.

**Temp file handling:** Both tesseract and markitdown-cli operate on file paths, not stdin. For content received as bytes (Telegram, Discord, HTTP), write to a temp file under `std::env::temp_dir().join("obsidian-borg/")`, process, then clean up. This is the same pattern used by `process_youtube_legacy()` for audio extraction.

**process_pdf()**:
```
PDF bytes
  -> store_asset(attachments/pdfs/)
  -> markitdown-cli extracts text to markdown
  -> Fabric summarizes (extract_article_wisdom or summarize pattern)
  -> Generate title from extracted content
  -> Generate tags
  -> render_note() with ![[pdf-embed]] link and summary
  -> Route by content topic
```

**process_audio()**:
```
audio bytes
  -> store_asset(attachments/audio/YYYY-MM/)
  -> Convert format if needed (OGG Opus -> MP3 via ffmpeg)
  -> Transcribe via existing Whisper infrastructure
  -> Classify transcription as text (reuse process_text logic)
  -> Generate title from transcription
  -> Generate tags
  -> render_note() with transcription body
  -> Route by content topic
```

**Audio format note:** Telegram voice notes arrive as OGG Opus. The existing Whisper integration supports MP3, WAV, and OGG. If the transcription service doesn't accept OGG directly, convert via ffmpeg (already a dependency for yt-dlp). Discord voice messages are also OGG Opus.

**process_text()**:
```
raw text
  -> Detect structured patterns:
     - "define: <word>" -> vocab definition note (language auto-detected)
     - "clarify: <word> vs <word>" -> vocab comparison note (language auto-detected)
     - URL detected -> redirect to process_url()
  -> If no pattern match:
     -> LLM classifies intent + topic (reuse obsidian_classify Fabric pattern)
     -> Generate title from content
     -> Generate tags
     -> render_note() with text as body
     -> Route: Football, Tech, Work, Writing, Spanish, Inbox, etc.
```

**process_document()**:
```
document bytes (docx, pptx, epub, etc.)
  -> store_asset(attachments/docs/)
  -> markitdown-cli extracts text
  -> Fabric summarizes
  -> Generate title, tags
  -> render_note() with ![[doc-embed]] link and summary
  -> Route by content topic
```

**Code detection (within process_text)**:

When `process_text()` determines the input looks like code (multi-line, indented, contains syntax markers), it switches to code rendering mode:
```
code text (detected within process_text)
  -> Detect language (heuristic: shebang, keywords, file markers; or ask LLM)
  -> Generate title from first comment/function/class name or LLM
  -> Tag with language + detected topic
  -> render_note() with fenced code block (```language ... ```)
  -> Route to 🤖 Tech/snippets/ or by detected domain
```

#### Structured Text Patterns

Two trigger prefixes for vocab notes. Both work for English and Spanish - the language is auto-detected from the word(s):

**`define: <word>`** - single word definition:
```
Input: "define: garrulous"
Language: English (detected)
Output note (🧠 Knowledge/english-vocab/garrulous.md):
---
title: "garrulous"
date: 2026-03-16
type: vocab
tags:
  - english-vocab
  - obsidian-borg
---

# garrulous

definition:: [LLM-generated definition]

## Examples

- [LLM-generated example sentences]
```

```
Input: "define: escurrir"
Language: Spanish (detected)
Output note (🇪🇸 Spanish/vocabulary/escurrir.md):
---
title: "escurrir"
date: 2026-03-16
type: vocab
tags:
  - spanish-vocab
  - obsidian-borg
---

# escurrir

definition:: [LLM-generated definition in context of Spanish learning]

## Examples

- [LLM-generated example sentences with translations]
```

**`clarify: <word> vs <word>`** - comparison/disambiguation:
```
Input: "clarify: escurrir vs estrujar"
Language: Spanish (detected)
Output note (🇪🇸 Spanish/vocabulary/escurrir-vs-estrujar.md):
---
title: "escurrir vs estrujar"
date: 2026-03-16
type: vocab
tags:
  - spanish-vocab
  - obsidian-borg
---

# escurrir vs estrujar

[LLM-generated comparison: definitions, usage contexts, examples, common confusions]
```

```
Input: "clarify: affect vs effect"
Language: English (detected)
Output note (🧠 Knowledge/english-vocab/affect-vs-effect.md):
---
title: "affect vs effect"
date: 2026-03-16
type: vocab
tags:
  - english-vocab
  - obsidian-borg
---

# affect vs effect

[LLM-generated comparison: definitions, usage contexts, examples, common confusions]
```

**Language detection:** The LLM determines the language from the word(s) provided. This is part of the same LLM call that generates the definition/comparison content, so there's no extra latency. The detected language determines both the routing folder and the tags.

These patterns are detected before general LLM classification, so dispatch is fast and deterministic. The LLM generates the note body content.

**Routing for structured notes is config-driven:**
```yaml
text_capture:
  vocab_folders:
    english: "🧠 Knowledge/english-vocab"
    spanish: "🇪🇸 Spanish/vocabulary"
    default: "🧠 Knowledge/vocab"    # fallback for other languages
```

This avoids hardcoding vault paths in Rust, consistent with how URL routing uses config-driven folders.

#### Asset Storage

Binary files (images, PDFs, audio, documents) are stored in the vault's existing attachment directory:

```
⚙️ System/attachments/
  images/YYYY-MM/         # screenshots, photos, diagrams
  pdfs/                   # PDF documents
  audio/YYYY-MM/          # voice memos, audio files
  docs/                   # docx, pptx, epub, etc.
```

The `YYYY-MM` subdirectory for images and audio prevents single-directory bloat. PDFs and docs are lower volume and don't need date bucketing.

**Asset path in frontmatter:**
```yaml
asset: "⚙️ System/attachments/images/2026-03/whiteboard-photo.png"
```

**Embedding in note body:**
```markdown
![[whiteboard-photo.png]]
```

Obsidian resolves `![[filename]]` links vault-wide, so the note doesn't need the full path for the embed. **Important:** filenames must be unique across the vault to avoid ambiguous wikilinks. The `store_asset()` function sanitizes filenames via `sanitize_filename()` and appends a short content hash suffix (first 8 chars of SHA-256) to guarantee uniqueness: `whiteboard-photo-a1b2c3d4.png`.

#### store_asset() Function

```rust
pub fn store_asset(
    vault_root: &Path,
    data: &[u8],
    filename: &str,
    subdirectory: &str,  // e.g. "images/2026-03", "pdfs"
) -> Result<PathBuf> {
    let attachments_dir = vault_root
        .join("⚙️ System")
        .join("attachments")
        .join(subdirectory);
    fs::create_dir_all(&attachments_dir)?;

    let dest = attachments_dir.join(filename);
    // Handle collision: append -1, -2, etc.
    let final_path = deduplicate_path(&dest);
    fs::write(&final_path, data)?;
    Ok(final_path)
}
```

### Data Model

#### Expanded `ContentType` enum (markdown.rs)

The existing `ContentType` in `markdown.rs` expands to cover all note types:

```rust
pub enum ContentType {
    YouTube { uploader: String, duration_secs: f64 },
    Article,
    Image { asset_path: String },
    Pdf { asset_path: String },
    Audio { asset_path: String, duration_secs: Option<f64> },
    Note,                              // plain text quick capture
    VocabDefine { word: String, language: String },                    // "define: <word>"
    VocabClarify { word_a: String, word_b: String, language: String }, // "clarify: <a> vs <b>"
    Document { asset_path: String },
    Code { language: String },
}
```

#### New `ContentKind` enum (types.rs)

This is the *input* classification (what did we receive), distinct from `ContentType` which is the *output* classification (what kind of note are we creating). Input sources construct this; the pipeline dispatches on it.

```rust
pub enum ContentKind {
    Url(String),
    Image { data: Vec<u8>, filename: String },
    Pdf { data: Vec<u8>, filename: String },
    Audio { data: Vec<u8>, filename: String },
    Text(String),
    Document { data: Vec<u8>, filename: String },
}
```

`Code` is not a `ContentKind` variant - code detection happens during `process_text()` sub-classification (see Architecture > Content Classification).

#### Updated `IngestRequest` (types.rs)

The HTTP API request type expands:

```rust
pub struct IngestRequest {
    // Existing fields
    pub url: Option<String>,  // was required, now optional
    pub tags: Option<Vec<String>>,
    pub priority: Option<Priority>,
    pub force: bool,
    pub method: Option<IngestMethod>,
    // New fields
    pub text: Option<String>,     // plain text content
    pub content_type: Option<String>, // hint: "image", "pdf", "audio", "code"
    // File data comes via multipart, not JSON
}
```

#### Updated frontmatter schema

New fields for non-URL content:

```yaml
---
title: "Note Title"
date: 2026-03-16
day: Monday
time: "14:30"
source: "https://url"           # URL content only
asset: "⚙️ System/attachments/images/2026-03/photo.png"  # file content only
type: youtube | article | image | pdf | audio | note | vocab | document | code
method: telegram | discord | http | clipboard | cli | ntfy
tags:
  - lowercase-hyphenated
# Type-specific fields
uploader: "Channel Name"        # youtube only
duration_min: 10                # youtube, audio
language: "rust"                # code only
---
```

Key change: `source` becomes optional (not all content has a URL), and `asset` is new (points to the stored binary file).

#### Updated `NoteContent` struct

```rust
pub struct NoteContent {
    pub title: String,
    pub source_url: Option<String>,  // was String, now optional
    pub asset_path: Option<String>,  // NEW: relative path to stored asset
    pub tags: Vec<String>,
    pub summary: String,
    pub content_type: ContentType,
    pub embed_code: Option<String>,
    pub method: Option<IngestMethod>,
}
```

#### Ledger changes

The ledger currently has a `Source` column for URLs. For non-URL content, this column stores a description instead:

| Date | Time | Method | Status | Title | Source | Folder |
|------|------|--------|--------|-------|--------|--------|
| 2026-03-16 | 14:30 | telegram | ✅ | [[photo note]] | [image: whiteboard-photo.png] | 🏈 Football |
| 2026-03-16 | 14:35 | cli | ✅ | [[garrulous]] | [define: garrulous] | 🧠 Knowledge/english-vocab |
| 2026-03-16 | 14:40 | telegram | ✅ | [[meeting note]] | [text: Met James at...] | 📥 Inbox |

The Source column serves as both the dedup key and the human-readable description. Dedup strategy per content type:
- **URLs:** Canonical URL match (existing behavior)
- **Files (image, PDF, audio, document):** SHA-256 content hash of the file bytes
- **Text notes:** No dedup. Quick captures are intentionally fire-and-forget; the user may send similar but distinct thoughts. If they send the exact same text, it's cheap to have a duplicate note in Inbox.
- **Vocab (`define:`, `clarify:`):** Dedup on the word/pair. Sending `define: garrulous` twice should update or skip, not create a duplicate.

### API Design

#### New top-level pipeline function

```rust
// The new entry point - replaces process_url as the primary pipeline
pub async fn process_content(
    content: ContentKind,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> IngestResult

// Existing function remains, called by process_content for URLs
pub async fn process_url(
    url: &str,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> IngestResult
```

#### New processing functions

```rust
// Image processing: OCR + vision + store asset
async fn process_image(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    config: &Config,
) -> Result<IngestResult>

// PDF processing: markitdown + summarize + store asset
async fn process_pdf(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    config: &Config,
) -> Result<IngestResult>

// Audio processing: transcribe + classify
async fn process_audio(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    config: &Config,
) -> Result<IngestResult>

// Text processing: pattern detection + LLM classification
async fn process_text(
    text: &str,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> Result<IngestResult>

// Document processing: markitdown + summarize + store asset
async fn process_document(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    config: &Config,
) -> Result<IngestResult>

// Code rendering is handled within process_text() when code is detected.
// No separate process_code() function - it shares the text pipeline.
```

#### Updated HTTP endpoints

```rust
// Existing - backward compatible
POST /ingest  (JSON body: IngestRequest)

// New - file upload
POST /ingest  (multipart/form-data: file + optional tags/force fields)

// New - text capture
POST /note    (JSON body: { text: String, tags: Option<Vec<String>> })
```

#### Updated CLI

```rust
pub enum Command {
    Daemon(DaemonOpts),
    Ingest {
        url: Option<String>,
        #[arg(long)]
        file: Option<PathBuf>,  // NEW: ingest a file
        #[arg(long)]
        clipboard: bool,
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        #[arg(long)]
        force: bool,
    },
    Note {                      // NEW: quick text capture
        text: Option<String>,
        #[arg(long)]
        clipboard: bool,
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    Hotkey(HotkeyOpts),
    Sign,
    Migrate { dry_run: bool, apply: bool },
}
```

#### OCR module (new: `ocr.rs`)

```rust
/// Extract text from image using tesseract
pub fn ocr_extract(image_path: &Path) -> Result<String>

/// Describe image using Claude vision API
pub async fn vision_describe(
    image_data: &[u8],
    mime_type: &str,
    config: &Config,
) -> Result<VisionResult>

pub struct VisionResult {
    pub description: String,
    pub suggested_title: String,
    pub suggested_tags: Vec<String>,
}
```

#### Document extraction module (new: `extraction.rs`)

```rust
/// Extract markdown from a file using markitdown-cli
pub fn extract_markdown(file_path: &Path) -> Result<String>

/// Extract markdown from PDF specifically
pub fn extract_pdf(file_path: &Path) -> Result<String>
```

### Implementation Plan

#### Phase 1: Pipeline Generalization

Introduce `ContentKind` enum and `process_content()` as the new top-level entry point. `process_url()` becomes an internal function called by `process_content()` for URL inputs. All existing callers (telegram, discord, routes, CLI) switch to `process_content()`.

At this point, non-URL inputs still get rejected - but the abstraction is in place.

Changes:
- `types.rs`: Add `ContentKind` enum
- `pipeline.rs`: Add `process_content()` that dispatches on `ContentKind`
- `markdown.rs`: Expand `ContentType` enum, make `source_url` optional in `NoteContent`
- `telegram.rs`, `discord.rs`, `routes.rs`, `lib.rs`: Update to call `process_content()`
- Tests for all existing URL behavior through new entry point

#### Phase 2: Plain Text Quick Capture

The highest-impact new capability. Accept text input from all sources and route intelligently.

Changes:
- `pipeline.rs`: Implement `process_text()`
- `pipeline.rs`: Add pattern detection for `define:` and `clarify:`
- `fabric.rs`: Add text classification function (reuse `obsidian_classify` pattern)
- `telegram.rs`: When no URL found in text, pass to `process_content(ContentKind::Text(...))`
- `discord.rs`: Same
- `cli.rs`: Add `Note` subcommand
- `routes.rs`: Add `POST /note` endpoint
- `markdown.rs`: Render `Note` and `Vocab` content types
- Tests for pattern detection, text routing, vocab note generation

#### Phase 3: Image Ingestion

Accept images, run OCR + vision, store originals, create searchable notes.

Changes:
- New module: `ocr.rs` (tesseract wrapper + Claude vision API)
- New module: `assets.rs` (`store_asset()` function)
- `pipeline.rs`: Implement `process_image()`
- `telegram.rs`: Handle `message.photo()` and image documents
- `discord.rs`: Handle image attachments
- `cli.rs`: `--file` flag on `ingest` detects image extensions
- `routes.rs`: Multipart form data support
- `markdown.rs`: Render `Image` content type with `![[embed]]`
- `config.rs`: Add `ocr` config section (tesseract path, vision API settings)
- `cli.rs`: Add `tesseract` to the `--help` required tools validation section (same pattern as yt-dlp, fabric, markitdown-cli)
- Tests for OCR extraction, asset storage, image note rendering

#### Phase 4: PDF and Document Ingestion

Accept PDFs and documents, extract text via markitdown, summarize.

Changes:
- New module: `extraction.rs` (markitdown-cli wrapper)
- `pipeline.rs`: Implement `process_pdf()` and `process_document()`
- `telegram.rs`: Handle document attachments by MIME type
- `discord.rs`: Handle document attachments
- `cli.rs`: `--file` flag detects PDF/document extensions
- `markdown.rs`: Render `Pdf` and `Document` content types
- Tests for text extraction, PDF note rendering

#### Phase 5: Audio / Voice Memo Ingestion

Accept audio files and voice notes, transcribe, create notes.

Changes:
- `pipeline.rs`: Implement `process_audio()`
- `telegram.rs`: Handle `message.voice()` and audio documents
- `discord.rs`: Handle audio attachments
- Reuse existing `TranscriptionClient` from `transcription.rs`
- `assets.rs`: Store audio files
- `markdown.rs`: Render `Audio` content type
- Tests for audio pipeline

#### Phase 6: Code Snippet Detection

Add code detection as a sub-path within `process_text()`.

Changes:
- `pipeline.rs`: Add code detection heuristic to `process_text()` (after pattern matching, before general LLM classification)
- `markdown.rs`: Render `Code` content type with fenced code blocks
- Route to `🤖 Tech/snippets/` or by detected domain
- Tests for language detection, code note rendering

This is the lowest priority phase because sending code via Telegram/Discord is uncommon. CLI is more natural for code (`obsidian-borg note --clipboard` with code on clipboard).

## Alternatives Considered

### Alternative 1: Separate binaries per content type

- **Description:** `obsidian-borg-image`, `obsidian-borg-pdf`, etc. as separate tools
- **Pros:** Smaller binaries, independent deployment, simpler code per tool
- **Cons:** Duplicates routing, ledger, config, tagging infrastructure. User manages multiple daemons. No unified input handling.
- **Why not chosen:** The value is in the unified pipeline. One daemon, one config, one ledger, one Telegram bot that handles everything.

### Alternative 2: Plugin architecture

- **Description:** Core pipeline with loadable plugins for each content type
- **Pros:** Extensible, content types can be added without modifying core
- **Cons:** Rust doesn't have a natural plugin model (dynamic loading is complex). Over-engineered for a personal tool with a known, finite set of content types.
- **Why not chosen:** YAGNI. The content types are known and stable. Adding a new one is a straightforward enum variant + processing function.

### Alternative 3: External preprocessing (shell scripts)

- **Description:** Shell scripts convert images/PDFs/audio to text, then pipe to existing URL-based pipeline
- **Pros:** No Rust changes needed, quick to prototype
- **Cons:** Loses metadata (original filename, MIME type), no asset storage, no proper frontmatter, brittle scripting around binary files
- **Why not chosen:** The pipeline needs to understand what it's processing to generate appropriate frontmatter, store assets, and route correctly.

### Alternative 4: Claude vision API only (no tesseract)

- **Description:** Use Claude's multimodal capabilities for all image processing, skip local OCR
- **Pros:** Single dependency, better understanding of image context
- **Cons:** Requires API call for every image (cost, latency), overkill for simple text extraction from screenshots, no offline capability
- **Why not chosen:** Hybrid approach is better. Tesseract handles simple OCR locally and fast. Vision API adds semantic understanding for images that benefit from it (diagrams, photos). Both outputs are combined.

### Alternative 5: Unified IngestRequest with base64 file data

- **Description:** Keep single JSON endpoint, encode files as base64 in the `file_data` field
- **Pros:** Simple API, no multipart parsing
- **Cons:** 33% size overhead from base64, large files bloat JSON, awkward for CLI/bot callers
- **Why not chosen:** Multipart is the standard for file uploads. The JSON endpoint remains for URL/text input. Both are needed.

## Technical Considerations

### Dependencies

**New crates:**
- None required for core functionality. tesseract and markitdown-cli are external binaries called via `Command`, same pattern as fabric and yt-dlp.

**Optional new crates:**
- `axum-multipart` or `multer` - for multipart form data parsing in the HTTP endpoint
- `sha2` - for content-hash dedup of binary files (already likely in dependency tree via other crates)
- `infer` - MIME type detection from file bytes (alternative to relying on sender-provided MIME)

**External tools (already available):**
- `tesseract` 5.3.4 - installed at `/usr/bin/tesseract`
- `markitdown-cli` - installed at `~/.local/bin/markitdown-cli`
- `fabric` - installed at `/home/saidler/go/bin/fabric`
- Claude API - already used for LLM classification

### Performance

- **OCR:** Tesseract is fast for single images (~1-3 seconds). Not a bottleneck.
- **Vision API:** Claude vision calls take 3-10 seconds. Can be optional/configurable for users who want faster processing.
- **markitdown-cli:** Fast for most documents. Large PDFs (100+ pages) may take a few seconds.
- **Whisper transcription:** Already handled by existing infrastructure. Voice memos are typically short (< 2 min), transcription is fast.
- **Asset storage:** Filesystem writes, negligible. Images from Telegram are typically 1-5MB.
- **LLM classification for text:** Already exists for URL routing. Same latency applies to text classification.

### Security

- **File uploads:** Validate MIME types and extensions. Reject executables and scripts. Maximum file size limit in config (default: 50MB).
- **Tesseract:** Runs on local files only, no network access. Input is validated image data.
- **markitdown-cli:** Runs on local files, similar to how fabric is invoked.
- **Vision API:** Sends image data to Claude API. Same trust model as existing LLM calls for summarization.
- **Stored assets:** Inherit vault permissions. No execution risk - they're data files rendered by Obsidian.

### Testing Strategy

- **Unit tests:** Content classification logic, pattern detection (`define:`, `clarity:`), filename sanitization for assets, content hash generation
- **Unit tests:** `render_note()` for each new `ContentType` variant
- **Unit tests:** Asset path generation and dedup
- **Integration tests:** Full pipeline for each content type (using test fixtures)
- **Integration tests:** Telegram/Discord message handling for photos, voice, documents, plain text
- **Integration tests:** Multipart HTTP upload
- **Integration tests:** CLI `--file` and `note` subcommand

### Rollout Plan

Phased rollout aligned with implementation phases:

1. **Phase 1** (pipeline generalization) - internal refactor, no user-visible changes
2. **Phase 2** (text capture) - Telegram/Discord users immediately benefit. CLI gets `note` command.
3. **Phase 3** (images) - send photos via Telegram, they become searchable notes
4. **Phase 4** (PDF/documents) - send files via Telegram, get summary notes
5. **Phase 5** (audio) - send voice memos via Telegram, get transcribed notes
6. **Phase 6** (code) - lowest priority, can ship whenever

Each phase is independently valuable and shippable. No phase depends on a later phase.

### Config Changes

New config sections:

```yaml
# OCR and vision settings
ocr:
  tesseract_binary: "tesseract"    # path to tesseract
  vision_enabled: true              # use Claude vision API for image description
  vision_model: "claude-sonnet-4-6" # model for vision calls

# Asset storage
assets:
  root: "⚙️ System/attachments"    # relative to vault root
  max_file_size_mb: 50              # reject files larger than this
  image_subdir: "images"            # images/{YYYY-MM}/
  pdf_subdir: "pdfs"
  audio_subdir: "audio"             # audio/{YYYY-MM}/
  docs_subdir: "docs"

# Text capture settings
text_capture:
  define_pattern: '^define:\s+(.+)$'        # single word definition
  clarify_pattern: '^clarify:\s+(.+)\s+vs\s+(.+)$'  # word comparison
  vocab_folders:
    english: "🧠 Knowledge/english-vocab"
    spanish: "🇪🇸 Spanish/vocabulary"
    default: "🧠 Knowledge/vocab"
  code_folder: "🤖 Tech/snippets"
```

### Migration Considerations

The migration config (`migration` section) will need updates for schema evolution:

```yaml
migration:
  field_renames:
    url: source    # existing
  field_transforms:
    source: canonicalize
    type: reclassify    # now handles: link -> youtube|article|image|pdf|audio|note|document|code
    tags: normalize
```

The `type: reclassify` transform gains awareness of new content types. Existing notes with `type: link` are still reclassified to `youtube` or `article` based on URL. New types only apply to newly ingested content.

### Dashboard Query Updates

The current Borg Dashboard uses `WHERE source != null` to find ingested notes. Non-URL notes (text, image, audio, etc.) won't have a `source` field. Update the dashboard queries to:

```dataview
WHERE source != null OR asset != null OR method != null
```

Using `method != null` is the most reliable discriminator - every obsidian-borg-created note has a `method` field, regardless of content type.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Tesseract OCR quality is poor for handwritten text | Medium | Low | Vision LLM provides fallback understanding. OCR is best-effort for searchability. |
| Large file uploads slow down Telegram bot | Low | Medium | Process files in background task (already using `tokio::spawn`). Send "Processing..." immediately. |
| Vision API costs increase with heavy image usage | Medium | Low | Make vision optional in config. Tesseract-only mode works without API calls. |
| markitdown-cli fails on exotic document formats | Low | Low | Graceful fallback - store asset and create minimal note with just the embed link. |
| Text classification routes notes to wrong folder | Medium | Low | Fallback to Inbox. User can move notes. Same risk as current URL classification. |
| Asset storage bloats vault git repo | Medium | Medium | Git LFS or .gitignore for attachments. Separate concern from obsidian-borg. |
| Telegram file download fails (network, file too large) | Low | Low | Standard error handling, log to ledger as failed. |
| Voice memo transcription quality varies | Low | Low | Whisper is good for clear speech. Quality is already proven for YouTube transcription. |
| `define:` / `clarify:` triggers on unintended text | Very Low | Low | Patterns are prefix-anchored and specific. False positives are unlikely. Easy to adjust patterns in config. |
| Breaking change to IngestRequest (url now optional) | Low | Medium | Existing callers always send url. Make it Option but log warning if both url and text are None. |
| Telegram message has both text and photo | Medium | Low | Photo takes priority (it's the richer content). Text becomes a tag hint or is prepended to the note body as context. |
| User sends a URL as part of a longer text message | Medium | Medium | Current behavior: extract URL, ignore surrounding text. New behavior should be: if text is *only* a URL, treat as URL. If text contains URL + other content, treat as Text and let process_text handle the URL within context. |
| Tesseract not installed on a deployment | Low | Medium | Graceful degradation: skip OCR, rely on vision LLM only. If neither is available, create note with just the image embed and no extracted text. Log warning. |
| Empty image (0 bytes) or corrupted file | Low | Low | Validate file size > 0 and basic magic bytes before processing. Reject with clear error. |
| Very long text input (novel-length paste) | Low | Low | Truncate for LLM classification (reuse existing `max_content_chars` from Fabric config). Full text still goes in the note body. |

## Open Questions

- [ ] Should `process_text()` have a minimum length threshold? (e.g., single words without `define:` prefix might be noise)
- [ ] Should vision LLM be used for ALL images, or only when tesseract returns low-confidence/empty text?
- [ ] For code snippets, should we try to detect language locally (tree-sitter) or just ask the LLM?
- [ ] Should asset filenames preserve the original name or use a sanitized slug? (e.g., `Sideline_Control.pdf` vs `sideline-control.pdf`)
- [ ] Content-hash dedup for files: SHA-256 of full file, or just filename + size? Full hash is correct but slower for large files.
- [ ] When a Telegram message has both a photo and a text caption, should the caption become: (a) the note title, (b) prepended to the note body, or (c) used as tag hints?
- [ ] Should `obsidian-borg note` send to the HTTP daemon (like `ingest` does) or process directly? Direct processing avoids the daemon requirement but means text capture only works if the binary is installed, not just the daemon running.
- [x] Are we solving the right problem? Yes - the vault already has multi-type content (PDFs, images, manual notes, vocab) but no formalized ingestion path. We're not changing how humans use the vault, just making the robot input channel handle everything.

## References

- Existing pipeline: `src/pipeline.rs` (process_url flow)
- Existing content types: `src/markdown.rs` (ContentType enum, render_note)
- Existing config: `src/config.rs` (Config struct)
- Existing types: `src/types.rs` (IngestRequest, IngestResult, ContentKind)
- Previous design doc: `docs/design/2026-03-07-canonicalization-dedup-dashboard.md`
- Vault structure: `~/repos/scottidler/obsidian/` (emoji-prefixed folders, templates, attachments)
- Vault templates: `⚙️ System/templates/` (vocab.md, note.md, book.md patterns)
- Tesseract: `/usr/bin/tesseract` (v5.3.4)
- markitdown-cli: `~/.local/bin/markitdown-cli`
- Fabric: `/home/saidler/go/bin/fabric`
