# Design Document: Direct Claude Vision API for Image Understanding

**Author:** Scott Idler
**Date:** 2026-03-16
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Replace the broken fabric-based vision path in `ocr.rs` with a direct call to the Anthropic Messages API. This gives obsidian-borg high-quality image understanding (text extraction, description, title/tag suggestions) for photos, screenshots, hardware labels, diagrams, and other visual content that tesseract handles poorly.

## Problem Statement

### Background

The image ingestion pipeline (Phase 3 of Universal Content Ingestion) has two OCR/vision paths:

1. **Tesseract** (`ocr_extract`) - local CLI, extracts raw text from images. Works well for clean printed text, screenshots with standard fonts.
2. **Vision LLM** (`vision_describe`) - intended to provide semantic understanding via Claude's multimodal API. Currently routes through `fabric --attachment`.

### Problem

1. **`vision_describe()` is broken.** It calls `fabric::run_pattern("summarize", &prompt)` passing the image *file path as text*. Fabric's `summarize` pattern ignores the `--attachment` flag entirely - the LLM never sees the image pixels.

2. **Fabric's `--attachment` flag is broken with Anthropic models.** Testing confirmed: `fabric --attachment <image> -m claude-sonnet-4-20250514` returns "empty response" for every Claude model. The same images work with `gpt-4o`. This is a fabric bug in how it constructs Anthropic API messages for image content.

3. **Tesseract alone is insufficient.** For the Netgate SG-2100 serial number sticker test, tesseract extracted garbled text (`115 a>`, `Inpul: 12V sez 20A0-@-O REG`) while GPT-4o via fabric extracted perfect results: model number, serial number, MAC address, FCC info, voltage ratings, and regulatory text. The quality gap is massive for anything beyond clean printed text.

4. **`vision_describe()` is never called.** The `process_image_inner()` function in `pipeline.rs` only calls `ocr_extract()` and never invokes `vision_describe()`. Even if the fabric path worked, it wouldn't be used.

### Goals

- Call the Anthropic Messages API directly with image bytes as base64, bypassing fabric entirely
- Extract text, description, title, and tags from images via Claude vision
- Use the existing `llm.api_key` config (already resolves `ANTHROPIC_API_KEY`)
- Integrate vision results into `process_image()` alongside tesseract OCR
- Make vision configurable (enable/disable, model selection) via config
- Graceful degradation: if vision fails or is disabled, fall back to tesseract-only

### Non-Goals

- Fixing the fabric `--attachment` bug (upstream issue)
- Replacing tesseract entirely (it's still useful for fast/free local extraction)
- Adding vision to non-image content types (PDFs, documents)
- Implementing image generation or editing
- Supporting non-Anthropic vision providers

## Proposed Solution

### Overview

Add a `vision_extract()` function to `ocr.rs` that calls the Anthropic Messages API directly via reqwest. The function sends image bytes as base64 in the message content, asks Claude to extract all visible text plus provide a description/title/tags, and parses the structured response. Wire this into `process_image_inner()` as a complement to tesseract.

### Architecture

```
Image bytes
    |
    +-- tesseract (local, fast, free)
    |       -> raw OCR text (best-effort)
    |
    +-- Claude Vision API (remote, 3-10s, paid)
    |       -> extracted text + description + title + tags
    |
    v
Merge results:
  - Title: vision title > OCR first line > filename
  - Text: vision extracted text (preferred) with OCR as supplement
  - Tags: vision tags + fabric tags
  - Description: vision description (new field in note body)
```

### API Design

#### New function in `ocr.rs`

```rust
/// Extract text and describe an image using the Claude Vision API directly.
///
/// Sends image bytes as base64 to the Anthropic Messages API.
/// Returns structured results or error if API key unavailable or call fails.
pub async fn vision_extract(
    image_data: &[u8],
    mime_type: &str,   // "image/jpeg", "image/png", etc.
    config: &Config,
) -> Result<VisionResult>
```

The function:
1. Resolves the API key via `config::resolve_secret(&config.api_key)`
2. Base64-encodes the image data
3. Sends a POST to `https://api.anthropic.com/v1/messages` with:
   - `model`: from config (default `claude-sonnet-4-20250514`)
   - `max_tokens`: 1024
   - `messages`: single user message with image content block + text prompt
4. Parses the structured response into `VisionResult`

#### Updated `VisionResult`

```rust
pub struct VisionResult {
    pub description: String,
    pub suggested_title: String,
    pub suggested_tags: Vec<String>,
    pub extracted_text: String,   // NEW: all text visible in image
}
```

#### Anthropic API request format

```json
{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "messages": [{
        "role": "user",
        "content": [
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": "<base64-encoded-image>"
                }
            },
            {
                "type": "text",
                "text": "Extract ALL text visible in this image and describe what you see.\n\nRespond in this exact format:\nTEXT: <all visible text, preserving layout>\nDESCRIPTION: <2-3 sentence description>\nTITLE: <3-8 word title>\nTAGS: <tag1>, <tag2>, <tag3>"
            }
        ]
    }]
}
```

#### Config changes

Add `VisionConfig` to `config.rs`:

```rust
pub struct VisionConfig {
    pub enabled: bool,           // default: true
    pub model: String,           // default: from llm.model
}
```

Add to `Config`:
```rust
pub vision: VisionConfig,
```

Config YAML:
```yaml
vision:
  enabled: true
  model: "claude-sonnet-4-20250514"  # optional override
```

#### Pipeline integration

In `process_image_inner()`, after tesseract OCR:

```rust
// Vision API extraction (best-effort, complements tesseract)
let vision = if config.vision.enabled {
    let mime = mime_from_extension(filename);
    match ocr::vision_extract(data, &mime, &config.llm).await {
        Ok(v) => Some(v),
        Err(e) => {
            log::warn!("Vision API failed: {e:#}");
            None
        }
    }
} else {
    None
};

// Merge results: vision preferred over tesseract
let title = vision.as_ref()
    .and_then(|v| (!v.suggested_title.is_empty()).then_some(&v.suggested_title))
    .cloned()
    .unwrap_or_else(|| /* existing tesseract/filename logic */);

let extracted_text = vision.as_ref()
    .and_then(|v| (!v.extracted_text.is_empty()).then_some(&v.extracted_text))
    .cloned()
    .unwrap_or_else(|| ocr_text.clone());
```

### Data Model

No schema changes. The vision results feed into the existing `NoteContent` fields:
- `title` - from `VisionResult.suggested_title`
- `summary` - includes both vision description and extracted text
- `tags` - merged from vision + fabric

### Implementation Plan

#### Phase 1: Direct API call

- Add `base64` crate to Cargo.toml (or use `data_encoding`)
- Implement `vision_extract()` in `ocr.rs` with direct reqwest call
- Add `VisionConfig` to config.rs
- Add `extracted_text` field to `VisionResult`
- Remove broken `vision_describe()` function
- Tests for response parsing

#### Phase 2: Pipeline integration

- Update `process_image_inner()` to call `vision_extract()`
- Merge vision + tesseract results (vision preferred)
- Include vision description in note body
- Include vision tags in tag set
- Tests for merge logic

## Alternatives Considered

### Alternative 1: Fix fabric's --attachment for Anthropic

- **Description:** Debug and fix the fabric Go codebase to properly handle --attachment with Anthropic models
- **Pros:** No new code in obsidian-borg, leverages existing fabric infrastructure
- **Cons:** Upstream dependency, unclear timeline, fabric's Anthropic image handling may have fundamental issues
- **Why not chosen:** We can't control fabric's release cycle, and the fix is trivial to implement directly

### Alternative 2: Use GPT-4o via fabric --attachment

- **Description:** Since fabric --attachment works with OpenAI models, use gpt-4o for vision
- **Pros:** Works today with zero code changes
- **Cons:** Requires OpenAI API key and billing, mixes providers, gpt-4o costs more than Claude for this use case
- **Why not chosen:** Already have Anthropic API key configured, adding a second provider adds complexity

### Alternative 3: Use anthropic Rust SDK crate

- **Description:** Add the `anthropic` Rust crate as a dependency
- **Pros:** Typed API, handles auth/retries
- **Cons:** Another dependency, may not support vision yet, version churn
- **Why not chosen:** The API call is simple enough (one POST with JSON) that reqwest is sufficient. The anthropic crate would add weight for minimal benefit.

## Technical Considerations

### Dependencies

**New:**
- `base64` crate (for encoding image bytes) - lightweight, stable

**Existing (already in Cargo.toml):**
- `reqwest` with `json` feature - for HTTP POST
- `serde_json` - for request/response serialization

### Performance

- Vision API call: 3-10 seconds depending on image size and model
- Base64 encoding: negligible (<1ms for a 5MB image)
- Runs concurrently with or after tesseract (tesseract is ~1-3s)
- Total image pipeline: ~5-12 seconds (acceptable for async ingestion)

### Security

- API key resolved via `config::resolve_secret()` (same as existing LLM/Groq keys)
- Image data sent to Anthropic API over HTTPS (same trust model as existing summarization calls)
- No secrets logged (existing `feedback_never_print_secrets` memory)

### Testing Strategy

- Unit test: parse well-formed vision API response
- Unit test: parse malformed/partial response (graceful degradation)
- Unit test: VisionResult merge logic (vision preferred over tesseract)
- Integration test: actual API call with a test image (gated behind env var or `#[ignore]`)

### Rollout Plan

1. Implement and test locally
2. Deploy with `vision.enabled: true` (default)
3. Send a test image via Telegram, verify vision results appear in note
4. Compare before/after for the Netgate SG-2100 serial number sticker

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Anthropic API rate limits on image calls | Low | Low | Single-image calls, not batch. Normal usage won't hit limits. |
| API cost increase from vision calls | Medium | Low | Vision is configurable (disable in config). Cost is ~$0.01-0.05 per image. |
| Large images exceed API size limit | Low | Medium | Anthropic supports up to 20MB base64. Telegram limits to 20MB. No action needed. |
| API response format changes | Very Low | Medium | Parse defensively with fallbacks for each field. |
| API key not configured | Low | Low | Graceful degradation - falls back to tesseract only. |

## Open Questions

- [x] Does fabric --attachment work with Claude? No - confirmed broken, empty response for all Claude models.
- [x] What's the Anthropic API format for image messages? Base64 in content block (documented above).
- [x] Should we run tesseract and vision in parallel (tokio::join) or sequentially? Yes - run in parallel. Tesseract in `spawn_blocking`, vision as async. Saves 1-3s per image.
- [ ] Should vision results be cached by content hash to avoid re-processing the same image? Not for v1 - premature optimization for personal use volumes.

## References

- Anthropic Messages API: https://docs.anthropic.com/en/api/messages
- Anthropic Vision docs: https://docs.anthropic.com/en/docs/build-with-claude/vision
- Existing LLM config: `src/config.rs` LlmConfig struct
- Broken vision function: `src/ocr.rs` vision_describe()
- Image pipeline: `src/pipeline.rs` process_image_inner()
- Fabric attachment test results: this conversation (fabric --attachment returns empty for all Claude models)
