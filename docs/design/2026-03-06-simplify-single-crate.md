# Design Document: Simplify obsidian-borg to a Single Crate

**Author:** Scott Idler
**Date:** 2026-03-06
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Collapse the obsidian-borg Cargo workspace (borg-core, borg-daemon, borg-transcriber) into a single Rust crate called `obsidian-borg`. obsidian-borg is an ingestion daemon that accepts any web link (articles, YouTube videos, etc.), extracts content, summarizes it via LLM, and writes Obsidian markdown notes. For YouTube transcription, it implements a four-tier fallback chain: YouTube auto-subs, cloud APIs (Groq/OpenAI), a remote GPU service (`youtube-siphon`, C/C++, separate repo), and local CPU inference. The Rust transcriber crate is removed — GPU transcription moves to `youtube-siphon`. With only one Rust binary remaining, the workspace is unnecessary overhead. This document covers merging the library code, removing the transcriber, flattening the directory structure, and simplifying the build configuration.

## Problem Statement

### Background

The workspace was introduced to separate the ingestion daemon from the Whisper transcription service, since they deploy to different machines with different dependency sets. The design doc addendum (2026-03-06) already concluded that the transcriber should be C/C++ — wrapping whisper.cpp through three layers of Rust FFI bindings was the wrong approach.

With the transcriber moving to its own repo (`youtube-siphon`), the workspace now contains:
- `borg-core` — a library crate with ~150 lines of shared config loading, error types, health handler, logging, and API types
- `borg-daemon` — the actual daemon, which is the only binary left
- `borg-transcriber` — dead code, superseded by the C/C++ repo

### Problem

A Cargo workspace with one real binary and a thin library crate is pointless complexity:
- `borg-core` exists only because the workspace needed shared types between two Rust binaries. With one binary, there's no sharing to do.
- The `crates/` directory structure adds indirection for no benefit.
- `.otto.yml` carries `--workspace` flags that are unnecessary for a single crate.
- New contributors must understand the workspace layout to find anything.

### Goals

- Single `Cargo.toml` at the repo root with `[package]` (not `[workspace]`)
- All source code under `src/` at the repo root
- Thin `main.rs` + `lib.rs` pattern for testability
- Binary name: `obsidian-borg`
- `.otto.yml` simplified for a single-crate Rust project
- All existing tests pass after restructure
- `borg-transcriber` crate removed entirely

### Non-Goals

- Implementing the full 4-tier transcription fallback chain (this doc covers the restructure; the fallback chain is documented here for context but implemented separately)
- Implementing new features (SQLite queue, LLM summarization, etc.)
- Building the youtube-siphon C/C++ service (separate repo)
- Changing the CI/CD pipeline beyond `.otto.yml` simplification

## Proposed Solution

### Overview

Flatten the workspace into a single crate. Merge `borg-core` modules into the crate's `lib.rs` module tree. Keep the daemon modules as-is. Delete `borg-transcriber` entirely.

### Architecture

```
obsidian-borg/              (before)                    (after)
├── Cargo.toml              [workspace]                 [package] name = "obsidian-borg"
├── build.rs                (none)                      git describe -> GIT_DESCRIBE
├── crates/                 3 crates                    (deleted)
│   ├── borg-core/          lib crate                   (merged into src/)
│   ├── borg-daemon/        bin crate                   (merged into src/)
│   └── borg-transcriber/   bin crate                   (deleted)
├── src/
│   ├── main.rs             (none)                      thin: parse CLI, call lib::run()
│   ├── lib.rs              (none)                      pub modules, build_router(), run_server()
│   ├── cli.rs                                          from borg-daemon (unchanged)
│   ├── config.rs                                       merged: loader + daemon config structs
│   ├── error.rs                                        from borg-core (unchanged)
│   ├── health.rs                                       from borg-core (unchanged)
│   ├── logging.rs                                      from borg-core (unchanged)
│   ├── types.rs                                        from borg-core (unchanged)
│   ├── routes.rs                                       from borg-daemon (updated imports)
│   ├── pipeline.rs                                     from borg-daemon (updated imports)
│   ├── url_router.rs                                   from borg-daemon (unchanged)
│   ├── youtube.rs                                      from borg-daemon (unchanged)
│   ├── jina.rs                                         from borg-daemon (unchanged)
│   ├── markdown.rs                                     from borg-daemon (unchanged)
│   └── transcription_client.rs                         from borg-daemon (updated imports)
├── docs/
├── .otto.yml               workspace flags             single-crate flags
└── build.rs                                            from borg-daemon/build.rs
```

### URL Handling and Transcription Fallback Chain

obsidian-borg accepts any web link. It classifies the URL and routes it through the appropriate pipeline:

- **YouTube / youtu.be links** — routed through the video pipeline (metadata via yt-dlp, transcript via fallback chain below, summary via LLM, output as Obsidian markdown)
- **All other URLs** — routed through the article pipeline (clean markdown via Jina Reader, summary via LLM, output as Obsidian markdown)

For YouTube videos that need transcription, obsidian-borg owns a four-tier fallback chain. Each tier is tried in order; failure at any tier transparently cascades to the next. The pipeline never breaks regardless of the state of external services.

| Tier | Path | When | Cost |
|------|------|------|------|
| **1. Fast Path** | `yt-dlp` auto-subs | Auto-captions available (~85% of videos) | Free, instant |
| **2. Cloud API** | Groq or OpenAI transcription API | No subs, cloud API configured | ~$0.006/min, 2-3 seconds |
| **3. Remote GPU** | `youtube-siphon` REST API (C/C++ on Windows+4090) | No subs, youtube-siphon configured and reachable | Free, 2-3 seconds |
| **4. Local CPU** | Shell out to local `whisper` CLI (openai-whisper via pipx) | All above failed or unconfigured | Free, slow (minutes) |

Tiers 2-3 are each independently optional — configured or not via `obsidian-borg.yml`. The daemon skips unconfigured tiers. Tier 4 is the final fallback: it shells out to the `whisper` command (same pattern as yt-dlp — subprocess call, not a Rust binding). If `whisper` is not installed or the call fails, the pipeline returns `IngestStatus::Failed` with a reason, signaling to the user that their ingestion failed for that URL.

The `transcription_client.rs` module owns this fallback logic. The types (`TranscriptionRequest`, `TranscriptionResponse`, `AudioFormat`) define the JSON contract used by Tiers 2-4. The youtube-siphon service (separate repo, C/C++) implements the same JSON API.

### Module Merge Details

**`config.rs`** — The two config files merge into one:
- Generic `load_config<T>()` function (from `borg-core`) moves into `config.rs` alongside the daemon-specific structs (`Config`, `ServerConfig`, `VaultConfig`, etc.)
- The `app_name` parameter becomes a constant `"obsidian-borg"`
- Tests from both files merge

**`lib.rs`** — Exports all public modules and contains `build_router()` and `run_server()`:
```rust
pub mod cli;
pub mod config;
pub mod error;
pub mod health;
pub mod jina;
pub mod logging;
pub mod markdown;
pub mod pipeline;
pub mod routes;
pub mod transcription_client;
pub mod types;
pub mod url_router;
pub mod youtube;

// build_router() and run_server() live here, extracted from old main.rs
```

**`main.rs`** — Thin entry point:
```rust
use clap::Parser;
use eyre::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    obsidian_borg::logging::setup_logging().context("Failed to setup logging")?;
    let cli = obsidian_borg::cli::Cli::parse();
    let config = obsidian_borg::config::load_config(cli.config.as_ref())
        .context("Failed to load configuration")?;
    obsidian_borg::run_server(config, cli.verbose).await
}
```

**Signature changes during merge:**
- `logging::setup_logging(app_name: &str)` drops the parameter, hardcodes `"obsidian-borg"` internally
- `config::load_config<T>(app_name: &str, path: Option<&PathBuf>)` drops `app_name`, hardcodes `"obsidian-borg"`. Keeps the generic `T: DeserializeOwned + Default` signature for testability (existing test uses a `TestConfig` struct), but the app name is no longer a parameter
- `health::health_handler(service, version)` — call site in `routes.rs` changes from `"borg-daemon"` to `"obsidian-borg"`
- `cli.rs` clap attribute `name = "borg-daemon"` becomes `name = "obsidian-borg"`, `about` and `after_help` updated to match
- `main.rs` log message `"Starting borg-daemon"` becomes `"Starting obsidian-borg"`
- `lib.rs` startup banner in `run_server()` changes from `"borg-daemon listening on"` to `"obsidian-borg listening on"`

**Import updates** — All `use borg_core::` references become `use crate::`:
- `borg_core::setup_logging(...)` -> `crate::logging::setup_logging()`
- `borg_core::load_config(...)` -> `crate::config::load_config(...)`
- `borg_core::HealthResponse` -> `crate::health::HealthResponse`
- `borg_core::health_handler` -> `crate::health::health_handler`
- `borg_core::types::*` -> `crate::types::*`

### Data Model

No changes. All types (`TranscriptionRequest`, `TranscriptionResponse`, `AudioFormat`, `IngestRequest`, `IngestResult`, `IngestStatus`, `Priority`) remain identical — they define the JSON contract with the remote C/C++ transcriber and HTTP clients.

### API Design

No changes. Same HTTP endpoints:

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/ingest` | Accept a URL for processing |
| `GET` | `/health` | Health check |

### Implementation Plan

#### Phase 1: Flatten Structure

- Create `src/` at repo root
- Copy all source files from `crates/borg-daemon/src/` and `crates/borg-core/src/` into `src/`
- Merge the two `config.rs` files
- Create thin `main.rs` + `lib.rs`
- Move `build.rs` from `crates/borg-daemon/` to repo root
- Write new root `Cargo.toml` as a single `[package]`
- Delete `crates/` directory entirely

#### Phase 2: Update Imports and Fix Compilation

- Replace all `borg_core::` imports with `crate::` imports
- Remove `borg-core` dependency from `Cargo.toml`
- Update `routes.rs` health handler to not reference `borg_core`
- Update `transcription_client.rs` to use `crate::types::*`
- Update `pipeline.rs` to use `crate::types::*`
- Verify `cargo check` passes

#### Phase 3: Simplify .otto.yml

| Task | Before | After |
|------|--------|-------|
| `check` | `cargo check --workspace --all-targets --all-features` | `cargo check --all-targets` |
| `check` | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | `cargo clippy --all-targets -- -D warnings` |
| `check` | `cargo fmt --all` / `cargo fmt --all --check` | `cargo fmt` / `cargo fmt --check` |
| `ensure` | `cargo test --all-targets --workspace -- --list` | `cargo test --all-targets -- --list` |
| `test` | `cargo test --workspace --all-features` | `cargo test` |
| `build` | `cargo build --release --workspace` | `cargo build --release` |

- Verify `otto ci` passes

#### Phase 4: Verify

- Run `cargo test` — all existing tests must pass
- Run `cargo clippy -- -D warnings` — clean
- Run `cargo fmt --check` — clean
- Run `otto ci` — green

## Alternatives Considered

### Alternative 1: Keep Workspace with Two Crates (drop transcriber only)

- **Description:** Remove `borg-transcriber` but keep `borg-core` as a library and `borg-daemon` as the binary.
- **Pros:** Keeps library/binary separation.
- **Cons:** `borg-core` has ~150 lines of code. The separation exists for a binary that no longer lives in this repo. It's complexity for the sake of structure, not utility.
- **Why not chosen:** One consumer of a library doesn't justify the library. Merge it.

### Alternative 2: Rust Bindings for Local Whisper (whisper-rs)

- **Description:** Use `whisper-rs` (Rust FFI bindings to whisper.cpp) for Tier 4 local CPU transcription instead of shelling out to the `whisper` CLI.
- **Pros:** No runtime dependency on the `whisper` Python package. Tighter integration, no subprocess overhead.
- **Cons:** Drags in `bindgen`, `cmake`, `clang-sys`, `libclang-dev` as build-time dependencies. Three layers of wrapping (whisper-rs -> whisper-rs-sys/bindgen -> whisper.cpp). Fragile build chain, slow compilation, and completely unnecessary when `Command::new("whisper")` works — the same pattern already used for yt-dlp. The original workspace design doc addendum reached the same conclusion for the GPU transcriber.
- **Why not chosen:** Shell out to the CLI. Keep it simple.

### Alternative 3: Rename borg-daemon to obsidian-borg, keep crates/ layout

- **Description:** Same workspace, just rename the binary crate and drop the transcriber.
- **Pros:** Minimal file moves.
- **Cons:** Still has the pointless `crates/` indirection and workspace Cargo.toml for a single crate.
- **Why not chosen:** If there's only one crate, there's no reason for `crates/`.

## Technical Considerations

### Dependencies

Single `Cargo.toml` combines deps from both `borg-core` and `borg-daemon`:

| Dependency | From | Purpose |
|-----------|------|---------|
| `axum` | both | HTTP server |
| `chrono` | daemon | timestamps in markdown |
| `clap` | daemon | CLI parsing |
| `colored` | daemon | terminal output |
| `dirs` | both | XDG directory discovery |
| `env_logger` | both | logging |
| `eyre` | both | error handling |
| `log` | both | logging facade |
| `reqwest` | daemon | HTTP client (Jina, Groq, transcriber) |
| `serde` | both | serialization |
| `serde_json` | both | JSON handling |
| `serde_yaml` | both | config file parsing |
| `thiserror` | core | error derive macro |
| `tokio` | both | async runtime |
| `tower` | daemon | middleware (test utilities) |
| `url` | daemon | URL parsing |

Removed: `borg-core` path dependency (no longer exists).

### Performance

No change. Same binary, same runtime behavior.

### Security

No change. Same attack surface, same Tailscale-only exposure.

### Testing Strategy

All existing tests migrate with their modules:
- `config.rs` — config loading defaults + deserialization (merged from both files)
- `error.rs` — (no tests currently)
- `health.rs` — health handler response
- `types.rs` — serialization roundtrips
- `routes.rs` — endpoint integration tests (via `tower::ServiceExt::oneshot`)
- `pipeline.rs` — tilde expansion
- `url_router.rs` — URL classification
- `youtube.rs` — VTT cleaning
- `jina.rs` — URL format
- `markdown.rs` — note rendering, filename sanitization
- `transcription_client.rs` — client construction

The thin `main.rs` has no logic to test. `lib.rs` exposes `build_router()` which the existing `routes.rs` tests already exercise.

### Rollout Plan

1. Single commit on a feature branch
2. `otto ci` green
3. Merge to main

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Import path mistakes during merge | Medium | Low | `cargo check` catches all of these at compile time |
| Missing dependency in unified Cargo.toml | Low | Low | `cargo check` catches immediately |
| Config app name change breaks existing config files | Medium | Medium | Config discovery path changes from `~/.config/borg-daemon/` to `~/.config/obsidian-borg/`. Document in commit message. Users move their config file. |
| Tests that depend on workspace structure | Low | Low | No tests reference workspace paths |

## Open Questions

- [x] ~~Should the config directory change from `borg-daemon` to `obsidian-borg`?~~ Yes. The binary is `obsidian-borg`, the config lives at `~/.config/obsidian-borg/obsidian-borg.yml`.
- [x] ~~Should TranscriptionRequest/Response types stay even though the transcriber is in another repo?~~ Yes. The daemon's `transcription_client.rs` uses them to serialize/deserialize the JSON contract. The types define the HTTP API, not a compile-time dependency.

## References

- [Workspace Architecture Design Doc](2026-03-06-workspace-architecture.md) — the original design being simplified
- [Cargo Package Layout](https://doc.rust-lang.org/cargo/guide/project-layout.html) — standard single-crate layout
