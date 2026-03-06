# Design Document: Obsidian Borg Workspace Architecture

**Author:** Scott Idler
**Date:** 2026-03-06
**Status:** Draft
**Review Passes Completed:** 5/5

## Summary

Restructure obsidian-borg from a single Cargo package into a Cargo workspace with three crates: a shared library (`borg-core`), the main ingestion daemon (`borg-daemon`), and a lightweight transcription microservice (`borg-transcriber`). This enables deploying the same monorepo to two machines with fundamentally different roles and dependency sets.

## Problem Statement

### Background

obsidian-borg is an ingestion pipeline that captures URLs (articles, YouTube videos) from any device and produces summarized Obsidian markdown notes. The architecture calls for two deployment targets:

1. **Ubuntu desktop** - The main daemon: receives URLs via webhook, routes them through yt-dlp or Jina Reader, summarizes via LLM, and writes `.md` files into the Obsidian vault.
2. **Windows 11 / WSL2 machine with RTX 4090** - A Whisper transcription microservice used as a fallback when YouTube auto-captions are unavailable (~15% of videos).

Both are Rust HTTP services, but they share very little beyond config loading and a common request/response contract.

### Problem

A single binary with config-driven "modes" would force both deployment targets to compile all dependencies, including GPU/Whisper bindings on the Ubuntu box and yt-dlp/LLM/Jina/Obsidian logic on the WSL2 box. This increases compile times, binary size, and attack surface unnecessarily.

### Goals

- Clean separation of concerns between the ingestion daemon and the transcription service
- Shared types enforce the API contract between the two services at compile time
- Each binary only compiles and ships the dependencies it actually needs
- A single repo for unified CI, versioning, and code review
- Straightforward `cargo build -p <crate>` for targeted builds

### Non-Goals

- Building a general-purpose plugin/extension system
- Supporting deployment targets beyond Ubuntu desktop and WSL2
- Implementing the full ingestion pipeline in this document (that's a separate design)
- Mobile app development or Android-side tooling
- Obsidian vault sync strategy (Syncthing, git, Obsidian Sync)

## Proposed Solution

### Overview

Convert the repo to a Cargo workspace with three member crates under `crates/`:

```
obsidian-borg/
├── Cargo.toml                  # workspace root
├── crates/
│   ├── borg-core/              # shared library
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── lib.rs
│   ├── borg-daemon/            # main ingestion daemon (Ubuntu)
│   │   ├── Cargo.toml
│   │   ├── build.rs
│   │   └── src/
│   │       ├── main.rs
│   │       ├── cli.rs
│   │       └── config.rs
│   └── borg-transcriber/       # Whisper microservice (WSL2)
│       ├── Cargo.toml
│       ├── build.rs
│       └── src/
│           ├── main.rs
│           ├── cli.rs
│           └── config.rs
├── docs/
├── .otto.yml
└── build.rs                    # removed (moves into each binary crate)
```

### Architecture

```
                ┌─────────────────────────────────────────────┐
                │              Cargo Workspace                │
                │                                             │
                │  ┌─────────────┐                            │
                │  │  borg-core  │  shared types, config,     │
                │  │   (lib)     │  error handling, HTTP      │
                │  │             │  client/server helpers     │
                │  └──────┬──────┘                            │
                │         │                                   │
                │    ┌────┴────┐                              │
                │    │         │                              │
                │  ┌─▼───────────┐  ┌──────────────────┐     │
                │  │ borg-daemon │  │ borg-transcriber  │     │
                │  │   (bin)     │  │      (bin)        │     │
                │  │             │  │                   │     │
                │  │ - HTTP srv  │  │ - HTTP srv        │     │
                │  │ - URL router│  │ - Whisper binding  │     │
                │  │ - yt-dlp    │  │ - Audio decode    │     │
                │  │ - Jina      │  │                   │     │
                │  │ - LLM client│  └───────────────────┘     │
                │  │ - MD writer │        WSL2 / 4090         │
                │  └─────────────┘                            │
                │    Ubuntu desktop                           │
                └─────────────────────────────────────────────┘

        Phone/Desktop ──HTTP POST──▶ borg-daemon
        borg-daemon ──HTTP POST──▶ borg-transcriber (when fallback needed)
                    ──HTTP POST──▶ Groq API (when transcriber unreachable)

#### Three-Tier Video Transcription Strategy

| Tier | Path | When | Cost |
|------|------|------|------|
| **1. Fast Path** | `yt-dlp` auto-subs | Auto-captions available (~85% of videos) | Free, instant |
| **2. Heavy Lift** | Audio -> `borg-transcriber` (4090) | No subs, WSL2 machine reachable | Free, 2-3 seconds |
| **3. Failsafe** | Audio -> Groq API | No subs, WSL2 unreachable/asleep | ~$0.006/min, 2-3 seconds |

The daemon owns this fallback chain. Each tier is tried in order; failure at any tier transparently cascades to the next. The pipeline never breaks regardless of the state of the WSL2 machine.
```

### Crate Responsibilities

#### `borg-core` (library)

Shared code used by both binaries:

| Concern | Contents |
|---------|----------|
| Config | Config loading (YAML, env, CLI), config file discovery chain |
| Types | `TranscriptionRequest`, `TranscriptionResponse`, `IngestRequest`, `IngestResult` |
| Errors | Shared error types via `thiserror` |
| Logging | Logging setup (file + env_logger) |
| HTTP helpers | Common Axum extractors, health check handler, middleware (request ID, logging) |

#### `borg-daemon` (binary)

The main ingestion service deployed on the Ubuntu desktop:

| Concern | Contents |
|---------|----------|
| HTTP server | Axum server with `/ingest` endpoint |
| URL routing | Detect YouTube vs article URLs, dispatch to appropriate pipeline |
| YouTube pipeline | Shell out to `yt-dlp` for metadata + subtitles, fall back to audio extraction |
| Article pipeline | Fetch clean markdown via Jina Reader (`r.jina.ai`) |
| Transcription client | Try `borg-transcriber` first; on connection failure, fall back to Groq API directly. Groq is called from the daemon, not the transcriber. |
| Queue | SQLite-backed job queue. `/ingest` writes a row and returns `Queued`. Background worker processes items. Survives restarts. |
| LLM summarization | Call Claude/Gemini/Ollama API with transcript or article text |
| Obsidian output | Write formatted `.md` with frontmatter, tags, embeds to vault Inbox |

#### `borg-transcriber` (binary)

A minimal Whisper transcription service deployed on the WSL2/4090 machine:

| Concern | Contents |
|---------|----------|
| HTTP server | Axum server with `POST /transcribe` endpoint |
| Audio decoding | Accept MP3/WAV/OGG via multipart upload, decode to PCM |
| Whisper inference | Run Whisper model via `whisper-rs` (CUDA-accelerated via whisper.cpp's cuBLAS backend) |
| Response | Return transcription text as JSON |

### Data Model

#### Shared types in `borg-core`

```rust
/// Sent from borg-daemon to borg-transcriber
pub struct TranscriptionRequest {
    pub audio_bytes: Vec<u8>,          // audio data (multipart upload)
    pub language: Option<String>,     // hint language code
    pub format: AudioFormat,          // mp3, wav, ogg
}

pub enum AudioFormat {
    Mp3,
    Wav,
    Ogg,
}

/// Returned from borg-transcriber to borg-daemon
pub struct TranscriptionResponse {
    pub text: String,
    pub language: String,
    pub duration_secs: f64,
}

/// Sent to borg-daemon's /ingest endpoint
pub struct IngestRequest {
    pub url: String,
    pub tags: Option<Vec<String>>,    // user-supplied tags
    pub priority: Option<Priority>,
}

pub enum Priority {
    Normal,
    High,  // skip queue, process immediately
}

/// Returned from /ingest
pub struct IngestResult {
    pub status: IngestStatus,
    pub note_path: Option<String>,
    pub title: Option<String>,
    pub tags: Vec<String>,
}

pub enum IngestStatus {
    Queued,
    Completed,
    Failed { reason: String },
}
```

#### Configuration

Each binary has its own config file but shares the config loading mechanism from `borg-core`:

**borg-daemon config** (`~/.config/borg-daemon/borg-daemon.yml`):

```yaml
server:
  host: "0.0.0.0"
  port: 8080

vault:
  inbox_path: "~/obsidian-vault/Inbox"

queue:
  db_path: "~/.local/share/borg-daemon/queue.db"

transcriber:
  url: "http://100.x.x.x:8090"  # Tailscale IP of WSL2 machine
  timeout_secs: 120

groq:
  api_key_env: "GROQ_API_KEY"
  model: "whisper-large-v3"

llm:
  provider: "claude"  # claude | gemini | ollama
  model: "claude-sonnet-4-6"
  api_key_env: "ANTHROPIC_API_KEY"

tagging:
  # Preferred tags the LLM should use when applicable
  preferred:
    - ai
    - rust
    - youtube
    - podcast
    - security
    - devops
  # Naming conventions the LLM must follow when generating new tags
  conventions:
    case: kebab     # lowercase, kebab-case
    max_tags: 5
    no_prefixes: true  # no redundant prefixes like "topic-" or "type-"
```

**borg-transcriber config** (`~/.config/borg-transcriber/borg-transcriber.yml`):

```yaml
server:
  host: "0.0.0.0"
  port: 8090

whisper:
  model: "large-v3"
  model_path: "~/.local/share/whisper/models"
  device: "cuda"       # cuda | cpu
  compute_type: "float16"
```

### API Design

#### borg-daemon

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/ingest` | Accept a URL for processing |
| `GET` | `/health` | Health check + queue depth |
| `GET` | `/status/:id` | Check processing status of a submitted URL |

#### borg-transcriber

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/transcribe` | Accept audio data, return transcription |
| `GET` | `/health` | Health check + model loaded status |

### Implementation Plan

#### Phase 1: Workspace Scaffolding

- Convert root `Cargo.toml` to `[workspace]`
- Create `crates/borg-core/`, `crates/borg-daemon/`, `crates/borg-transcriber/`
- Move existing scaffold code into `borg-daemon`
- Extract config loading and logging into `borg-core`
- Add shared types (empty structs initially) to `borg-core`
- Update `.otto.yml` to work with workspace (add `--workspace` to `cargo check`, `cargo clippy`, `cargo fmt --all`, `cargo test`)
- Verify `otto ci` passes

#### Phase 2: HTTP Foundation

- Add Axum to `borg-core` as a shared dependency
- Implement health check handler in `borg-core`
- Stand up `borg-daemon` HTTP server with `/ingest` (stub) and `/health`
- Stand up `borg-transcriber` HTTP server with `/transcribe` (stub) and `/health`
- Add integration tests for both servers starting and responding

#### Phase 3: Transcription Service

- Add `whisper-rs` to `borg-transcriber`
- Implement audio decoding (accept multipart upload or raw bytes)
- Implement Whisper inference with CUDA support
- Add a CPU fallback mode for development/testing
- Test with sample audio files

#### Phase 4: Ingestion Pipeline

- Implement URL routing (YouTube vs article detection)
- Integrate `yt-dlp` subprocess calls (metadata, subtitles, audio extraction)
- Integrate Jina Reader for article markdown
- Implement transcription client in `borg-daemon` (calls `borg-transcriber`)
- Add Groq API as an alternative transcription backend
- Implement LLM summarization client
- Implement Obsidian markdown output (frontmatter, tags, embeds)
- End-to-end tests with mock services

#### Phase 5: Deployment and Networking

- Document Tailscale setup for both machines
- Create systemd unit files for both services
- Add HTTP Shortcuts configuration guide for Android
- Add desktop hotkey script for Ubuntu
- Test full flow: phone share -> daemon -> transcriber -> vault

## Alternatives Considered

### Alternative 1: Single Binary with Feature Flags

- **Description:** One binary, use Cargo features (`--features daemon` / `--features transcriber`) to conditionally compile each mode.
- **Pros:** Single crate, simpler Cargo.toml, one build artifact to version.
- **Cons:** Feature flags add conditional compilation complexity (`#[cfg(feature = "...")]` scattered through code). Both feature sets still exist in the same dependency tree during development. Easy to accidentally leak daemon deps into transcriber builds or vice versa. The two modes share ~20% of code, not enough to justify coupling.
- **Why not chosen:** Workspace gives cleaner separation with less ceremony than feature flags for this level of divergence.

### Alternative 2: Single Binary with Config-Driven Mode

- **Description:** One binary that reads a `mode: daemon | transcriber` field from config and only starts the relevant functionality.
- **Pros:** Simplest deployment story (one binary everywhere).
- **Cons:** The binary always compiles all dependencies. The WSL2 machine would ship with yt-dlp orchestration code and LLM client code it never uses. The Ubuntu machine would ship with Whisper/CUDA bindings it never uses. Increases binary size and compile time on both targets.
- **Why not chosen:** Violates the principle of least privilege for dependencies. The 4090 box doesn't need LLM clients; the Ubuntu box doesn't need CUDA bindings.

### Alternative 3: Separate Repositories

- **Description:** Two independent repos: `obsidian-borg` (daemon) and `borg-transcriber`.
- **Pros:** Complete isolation.
- **Cons:** Shared types must be published as a separate crate or duplicated. API contract changes require coordinated releases across repos. Harder to keep in sync. More CI configuration to maintain.
- **Why not chosen:** A workspace gives the same build isolation while keeping shared types in sync at compile time and using a single CI pipeline.

## Technical Considerations

### Dependencies

| Crate | Key Dependencies |
|-------|-----------------|
| `borg-core` | `serde`, `serde_yaml`, `tokio`, `axum`, `thiserror`, `tracing`, `dirs` |
| `borg-daemon` | `borg-core`, `reqwest`, `rusqlite`, `clap` |
| `borg-transcriber` | `borg-core`, `whisper-rs`, `clap` |

Note: `borg-daemon` shells out to `yt-dlp` (YouTube metadata + subtitles + audio extraction) but uses `reqwest` for all HTTP calls: Jina Reader, transcriber, LLM APIs, and Groq fallback.

External runtime dependencies:
- `yt-dlp` (system package)
- Whisper model files (~1.5GB for large-v3)
- Tailscale (for cross-machine networking)

### Performance

- **borg-daemon**: I/O bound. Most time spent waiting on yt-dlp, Jina, LLM API, or the transcriber. Axum with Tokio handles concurrency naturally. Ingest requests are written to SQLite immediately and return `Queued`; a background worker polls the queue and processes items. This survives daemon restarts and enables `/status/:id` lookups.
- **borg-transcriber**: GPU bound. Whisper inference on a 4090 takes 2-3 seconds for typical audio. Sequential processing is fine; concurrent requests queue behind the GPU. A simple `tokio::sync::Semaphore` with 1 permit prevents GPU OOM. Request body size is capped at 100MB to handle long podcast audio (~1 hour MP3 at 128kbps = ~57MB).

### Security

- Both services bind to `0.0.0.0` but are only reachable via Tailscale (100.x.x.x network). No public internet exposure.
- No authentication between services initially. Tailscale's WireGuard encryption and identity provides the trust boundary.
- LLM API keys are read from environment variables, never stored in config files.
- `yt-dlp` URLs are validated before being passed to subprocess calls to prevent command injection.

### Testing Strategy

| Level | Scope |
|-------|-------|
| Unit | Config parsing, URL routing logic, markdown generation, type serialization |
| Integration | Each HTTP server starts and responds correctly to health checks |
| Contract | `borg-daemon` can serialize a `TranscriptionRequest` that `borg-transcriber` can deserialize (and vice versa for the response). Run via `cargo test --workspace`. |
| End-to-end | Mock yt-dlp and Jina responses, verify full pipeline produces expected `.md` output |

### Rollout Plan

1. **Phase 1 (workspace scaffolding)** is a pure restructure, no behavior change
2. Each subsequent phase is independently deployable
3. `borg-transcriber` can be developed and tested standalone with audio files before `borg-daemon` integration
4. Both services use graceful shutdown (Axum's `with_graceful_shutdown`)

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `whisper-rs` CUDA build issues on WSL2 | Medium | High | Test WSL2 CUDA passthrough early in Phase 3. Fall back to Groq API if needed. |
| `yt-dlp` breaking changes from YouTube | Medium | Medium | Pin `yt-dlp` version, update on a cadence. yt-dlp community patches breakages within hours. |
| WSL2 machine off or asleep when transcription needed | Medium | Low | `borg-daemon` detects connection failure and automatically falls back to Groq API. Transparent to the user. |
| Tailscale not available (phone on restricted network) | Low | Medium | Queue requests locally on phone via HTTP Shortcuts retry. Daemon processes when reachable. |
| Whisper model too large for WSL2 memory | Low | Low | large-v3 needs ~2GB VRAM; 4090 has 24GB. Use medium model if needed. |
| Scope creep into full note-taking app | Medium | High | Non-goals are explicit. This is an ingestion pipeline only. |

## Open Questions

- [x] ~~Should `borg-transcriber` accept audio via multipart upload or expect the daemon to provide a URL it can fetch from?~~ Multipart upload. The daemon already has the audio from yt-dlp; no reason to make the transcriber fetch it again.
- [x] ~~Should we support Groq API as a transcription backend directly in `borg-daemon` (bypassing `borg-transcriber` entirely) or route all transcription through the transcriber which itself can delegate to Groq?~~ Groq is called directly from `borg-daemon`. The transcriber stays simple (Whisper only). The daemon owns the fallback logic: try transcriber first, fail over to Groq.
- [x] ~~What Whisper model size gives the best speed/quality tradeoff on the 4090 for typical 10-30 min YouTube videos?~~ `large-v3`. With 24GB VRAM on the 4090, there's no reason to compromise. `large-v3` processes in seconds and has significantly lower hallucination rates than `medium`/`base`, especially for technical content and varying accents. Consider `large-v3-turbo` if available for even faster inference at the same quality tier.
- [x] ~~Should the daemon persist a processing queue to disk (SQLite) or is an in-memory Tokio channel sufficient?~~ SQLite. Survives daemon restarts, enables `/status/:id` lookups, and provides a history of processed URLs.
- [x] ~~Tag schema: should tags come from a fixed vocabulary file or be fully LLM-generated?~~ Hybrid. Common/canonical tags are defined in the config file (e.g. `#ai`, `#rust`, `#youtube`). The LLM is also free to generate new tags, but is guided by schema and naming conventions (e.g. lowercase, kebab-case, no redundant prefixes). The config tags act as a preferred vocabulary, not a hard constraint.

## Addendum: borg-transcriber Should Be C++ (2026-03-06)

During implementation it became clear that wrapping whisper.cpp in Rust via `whisper-rs` is the wrong call. The current Rust transcriber exists as a stub in the workspace but should be rewritten as a native C++ service before real deployment.

### Why Rust is wrong for this crate

The transcriber does exactly three things: accept audio over HTTP, run whisper.cpp inference, return JSON. whisper.cpp is already C++. The Rust path to get there is:

```
Rust code -> whisper-rs (safe wrapper) -> whisper-rs-sys (bindgen FFI) -> whisper.cpp (C++)
                                               |
                                          build.rs runs cmake to compile whisper.cpp
                                          bindgen + libclang to generate extern "C" bindings
```

Three layers of wrapping to call a C++ library. And the build chain drags in `bindgen`, `cmake`, `clang-sys`, `libclang-dev`, `whisper-rs-sys` — all fragile, all slow, all unnecessary when you could just... write C++.

### Why it doesn't matter for the workspace

The daemon talks to the transcriber over HTTP with JSON. The "shared types" in `borg-core` (`TranscriptionRequest`, `TranscriptionResponse`) define a JSON schema, not a compile-time contract. The daemon doesn't care what language the transcriber is written in. A C++ service returning the same JSON is indistinguishable from the Rust stub.

### Proposed approach

- Replace `crates/borg-transcriber/` with a C++ project (cmake-based)
- Use [cpp-httplib](https://github.com/yhirose/cpp-httplib) (header-only HTTP server) or similar
- Call whisper.cpp directly — no FFI, no bindgen, no Rust in the middle
- Link cuBLAS/CUDA natively for the 4090
- Same JSON API contract, same systemd unit, same Tailscale networking
- Could live at `services/borg-transcriber/` to distinguish from Rust crates

### What stays in Rust

Everything on the daemon side: `borg-core` (shared types/config), `borg-daemon` (HTTP server, pipeline, yt-dlp, Jina, Groq fallback, markdown output). Rust is the right choice there — it's an I/O-bound async service with many integrations. The transcriber is a GPU-bound C++ wrapper and should be written accordingly.

## References

- [Obsidian Ingestion Ideas](../obsidian-ingestion-ideas.md) - Original brainstorm document
- [Cargo Workspaces](https://doc.rust-lang.org/book/ch14-03-cargo-workspaces.html) - Rust book reference
- [whisper-rs](https://github.com/tazz4843/whisper-rs) - Rust bindings for whisper.cpp
- [Axum](https://github.com/tokio-rs/axum) - Web framework
- [Jina Reader](https://r.jina.ai) - URL-to-markdown service
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) - YouTube downloader
