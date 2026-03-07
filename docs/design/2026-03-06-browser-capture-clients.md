# Design Document: Browser Capture Clients

**Author:** Scott Idler
**Date:** 2026-03-06
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add three browser-based capture methods (bookmarklet, WebExtension, global hotkey script), a CLI `ingest` subcommand, and `install`/`uninstall` subcommands for daemon lifecycle management. All capture clients POST to the existing HTTP `/ingest` endpoint. The Rust binary gains clap subcommands (`serve`, `ingest`, `install`, `uninstall`) while remaining a single crate. Secret resolution is unified: config values are auto-detected as file paths or env var names.

## Problem Statement

### Background

obsidian-borg currently supports three ingestion pathways: Telegram bot, Discord bot, and a raw HTTP `POST /ingest` endpoint. Telegram works well for mobile and cross-device capture. However, when browsing on the desktop (watching YouTube, reading articles), there is no low-friction way to signal a URL for ingestion without leaving the browser, opening Telegram, and pasting the link.

The HTTP endpoint already exists and works. What's missing are **clients** that make it easy to invoke from a browser context.

### Problem

Desktop browser capture requires too many steps: copy URL, switch to Telegram, paste, send. We need one-click or one-hotkey capture directly from the browser.

### Goals

- One-click capture from any browser via bookmarklet
- One-click or one-hotkey capture via browser extension with success/failure feedback
- System-wide hotkey capture via clipboard-based shell script
- CLI subcommand `obsidian-borg ingest <URL>` that reads config for endpoint
- All clients respect `server.host` and `server.port` from `obsidian-borg.yml`

### Non-Goals

- Mobile capture (Telegram already handles this)
- Browser history or tab syncing
- New Rust ingestion pathways (no new axum routes)
- Android HTTP Shortcuts / Tasker integration (future work)
- Read-it-later app integration (Omnivore/Wallabag)

## Proposed Solution

### Overview

Four new clients, all targeting the existing `POST /ingest` endpoint:

| Client | Location | Language | Config Source |
|--------|----------|----------|---------------|
| Bookmarklet | `clients/bookmarklet/` | JavaScript | User edits URL in snippet |
| WebExtension | `clients/extension/` | JS (Manifest V3) | Auto-discovers via `/health`, fallback to options page |
| Hotkey script | `clients/hotkey/` | Bash | Reads `obsidian-borg.yml` via `obsidian-borg ingest` |
| CLI subcommand | `src/cli.rs` | Rust | Reads `obsidian-borg.yml` directly |

Plus a small Rust change: clap subcommands on the existing binary.

### Architecture

**Full ingestion pathway map** (existing + new):

```
  MOBILE                      DESKTOP BROWSER                    TERMINAL
  ------                      ---------------                    --------
  Telegram bot (existing)     Bookmarklet (new)                  CLI ingest (new)
  Discord bot  (existing)     WebExtension (new)
                              Hotkey script (new)
       |                           |                                |
       |   +--- Telegram API ---+  |                                |
       |   +--- Discord GW  ---+  |                                |
       v   v                   v  v                                v
  +--------+---------------------+----------------------------------+
  |                    obsidian-borg daemon                          |
  |                                                                 |
  |  Telegram handler -+                                            |
  |  Discord handler --+--> POST /ingest --> pipeline --> vault     |
  |  HTTP /ingest -----+                                            |
  |  GET /health (discovery)                                        |
  +-----------------------------------------------------------------+
```

All new clients are HTTP-only — they POST to `/ingest` on the running daemon. No new server-side routes or protocols needed.

**New clients detail:**

```
         bookmarklet    extension     hotkey script    CLI ingest
         (browser JS)  (Manifest V3) (bash+xclip)    (Rust HTTP client)
              |              |              |                |
              |         auto-discover       |                |
              |         via /health         |                |
              |         fallback:           |                |
              |         options page        |                |
              |              |              |                |
              +--------------+--------------+----------------+
                     all POST {url: "..."} to /ingest
```

### Unified Secret Resolution

Config keys for secrets (API keys, bot tokens) accept **either** a file path or an environment variable name. The daemon auto-detects at startup:

1. If the value is a path to an existing file, read its contents as the secret
2. Otherwise, treat the value as an environment variable name and resolve from env

```yaml
# Secret values: if the path exists, read the file; otherwise treat as env var name.
#
# File path example:  "~/.config/telegram/api-token"   -> reads file contents
# Env var example:    "TELEGRAM_BOT_TOKEN"              -> reads $TELEGRAM_BOT_TOKEN

telegram:
  bot_token: "~/.config/telegram/api-token"

groq:
  api_key: "~/.config/groq/api-token"

llm:
  api_key: "~/.config/anthropic/api-token"
```

This aligns with the existing secret management pattern (`~/.config/<company>/api-token` files exported via `~/.shell-exports.d/`). The daemon reads the files directly, so it works under systemd without needing shell exports or EnvironmentFile directives.

**Implementation:** A small `resolve_secret()` helper (~10 lines):

```rust
fn resolve_secret(value: &str) -> Result<String> {
    let expanded = shellexpand::tilde(value);
    let path = Path::new(expanded.as_ref());
    if path.exists() {
        Ok(fs::read_to_string(path)?.trim().to_string())
    } else {
        std::env::var(value)
            .context(format!("secret '{value}' is not a file and env var is not set"))
    }
}
```

**Migration:** Rename existing `_env` suffixed config keys:

| Before | After |
|--------|-------|
| `bot_token_env: "TELEGRAM_BOT_TOKEN"` | `bot_token: "~/.config/telegram/api-token"` |
| `api_key_env: "GROQ_API_KEY"` | `api_key: "~/.config/groq/api-token"` |
| `api_key_env: "ANTHROPIC_API_KEY"` | `api_key: "~/.config/anthropic/api-token"` |

Both old env var names (`"GROQ_API_KEY"`) and new file paths work as values — the resolver handles both transparently.

**New Rust dependency:** `shellexpand` (for `~` expansion in secret paths).

### CLI Subcommand Design

Current CLI (flat, no subcommands):

```
obsidian-borg [--config PATH] [--verbose] [--log-level LEVEL]
```

Proposed CLI (subcommands, backward-compatible):

```
obsidian-borg                              # default: runs daemon (serve)
obsidian-borg serve                        # explicit: runs daemon
obsidian-borg ingest <URL> [--tags t1,t2]  # posts URL to running daemon
obsidian-borg install [--force]            # install system service
obsidian-borg uninstall                    # remove system service
```

The `ingest` subcommand:
1. Loads config from `obsidian-borg.yml` (same resolution logic as daemon)
2. Reads `server.host` and `server.port` to build the endpoint URL
3. POSTs `{"url": "<URL>", "tags": [...]}` to `http://{host}:{port}/ingest`
4. On success: prints a human-readable line to stdout (e.g., `Captured: "Deep Dive into LLMs" -> Inbox/deep-dive-into-llms.md`)
5. On failure: prints error to stderr
6. Exits with 0 on success, 1 on failure

The human-readable output is intentional — the hotkey script captures it for `notify-send`. Machine-readable JSON output (`--json` flag) can be added later if needed.

**Error handling:** If the daemon is unreachable (connection refused), print a clear message: `Error: cannot reach obsidian-borg at http://{host}:{port} — is the daemon running?` rather than exposing raw reqwest errors.

**Config fallback:** If no config file is found, `ingest` falls back to `localhost:8181` (the `ServerConfig::default()`). The CLI only needs host:port, unlike the daemon which requires the full config.

Implementation in `src/cli.rs`:

```rust
#[derive(Parser)]
#[command(name = "obsidian-borg", ...)]
pub struct Cli {
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    #[arg(short, long)]
    pub verbose: bool,

    #[arg(short, long)]
    pub log_level: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the ingestion daemon (default)
    Serve,
    /// Send a URL to the running daemon for ingestion
    Ingest {
        /// URL to ingest
        url: String,
        /// Comma-separated tags
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    /// Install as a system service (systemd on Linux, launchd on macOS)
    Install {
        /// Overwrite existing service file
        #[arg(long)]
        force: bool,
    },
    /// Remove the system service
    Uninstall,
}
```

In `main.rs`:

```rust
match cli.command {
    None | Some(Command::Serve) => obsidian_borg::run_server(config, cli.verbose).await,
    Some(Command::Ingest { url, tags }) => obsidian_borg::run_ingest(config, url, tags).await,
    Some(Command::Install { force }) => obsidian_borg::install_service(force).await,
    Some(Command::Uninstall) => obsidian_borg::uninstall_service().await,
}
```

`run_ingest` is a small function in `lib.rs` (~20 lines) that builds the URL from config, POSTs via reqwest, prints the result.

### Install / Uninstall Subcommands

**`obsidian-borg install`** generates and installs a platform-appropriate service definition:

**Linux (systemd):**

1. Detects binary path via `std::env::current_exe()`
2. Generates a hardened systemd unit:

```ini
[Unit]
Description=obsidian-borg - Obsidian ingestion daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=%u
ExecStart=/path/to/obsidian-borg serve
Restart=always
RestartSec=5
StartLimitBurst=5
StartLimitIntervalSec=60
WorkingDirectory=%h

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=%h/repos/scottidler/obsidian
PrivateTmp=true

[Install]
WantedBy=default.target
```

3. Writes to `~/.config/systemd/user/obsidian-borg.service` (user unit — no sudo required)
4. Runs `systemctl --user daemon-reload && systemctl --user enable --now obsidian-borg`
5. Prints status confirmation

**Key hardening directives:**

| Directive | Purpose |
|-----------|---------|
| `Restart=always` | Restarts on any exit, not just failure |
| `StartLimitBurst=5` / `StartLimitIntervalSec=60` | Prevents crash loops (max 5 restarts per minute) |
| `NoNewPrivileges=true` | Process can't escalate privileges |
| `ProtectSystem=strict` | Filesystem is read-only except explicit paths |
| `ProtectHome=read-only` | Home dir is read-only except explicit paths |
| `ReadWritePaths=` | Only the vault inbox is writable |
| `PrivateTmp=true` | Isolated /tmp |

**Why user unit instead of system unit:** No `sudo` required. Starts when the user logs in (which is the right behavior for a desktop daemon). The existing `deploy/obsidian-borg.service` was a system unit requiring `sudo` and manual API key management — this replaces it cleanly.

**Note on secrets:** Because the daemon now reads secret files directly via `resolve_secret()`, the systemd unit needs no `Environment=` or `EnvironmentFile=` directives for API keys. The config points to `~/.config/<company>/api-token` files, and `ProtectHome=read-only` allows reading them.

**macOS (launchd):**

1. Generates a plist:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "...">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.obsidian-borg</string>
    <key>ProgramArguments</key>
    <array>
        <string>/path/to/obsidian-borg</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/tmp/obsidian-borg.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>/tmp/obsidian-borg.stderr.log</string>
</dict>
</plist>
```

2. Writes to `~/Library/LaunchAgents/com.obsidian-borg.plist`
3. Runs `launchctl load ~/Library/LaunchAgents/com.obsidian-borg.plist`

**`obsidian-borg uninstall`** reverses the process:
- Linux: `systemctl --user disable --now obsidian-borg`, removes the unit file, `daemon-reload`
- macOS: `launchctl unload`, removes the plist

### Bookmarklet

A JavaScript one-liner saved as a browser bookmark:

```javascript
javascript:void(fetch('http://localhost:8181/ingest',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({url:location.href})}).then(r=>r.json()).then(d=>alert(d.title||'Sent!')).catch(e=>alert('Error: '+e)))
```

Delivered as a `README.md` with:
- The snippet (user edits `localhost:8181` if their port differs)
- Drag-to-bookmarks-bar install instructions
- Note about CORS (bookmarklet runs in page context; the daemon may need to allow `Origin` headers — see Technical Considerations)

### WebExtension (Manifest V3)

**Files:**

```
clients/extension/
  manifest.json
  background.js      # service worker: handles click + shortcut
  popup.html         # minimal status popup
  popup.js           # popup logic
  options.html       # settings page (fallback config)
  options.js         # saves/loads endpoint URL
  icons/
    icon-16.png
    icon-48.png
    icon-128.png
```

**Behavior:**

1. On install, `background.js` tries `fetch('http://localhost:8181/health')` (hardcoded default).
2. If response JSON has `service === "obsidian-borg"`, stores `http://localhost:8181` as the endpoint in `chrome.storage.local`. No port scanning — just the one default attempt.
3. If discovery fails (daemon not running, wrong port, network error), opens the options page for manual configuration. User enters the correct `host:port`.
4. On toolbar click or `Alt+Shift+B`: gets active tab URL, POSTs to `/ingest`, shows badge/notification with result.
5. Popup shows last ingestion result (title, status, tags).

**Manifest V3 key points:**
- `permissions: ["activeTab", "storage", "notifications"]`
- `host_permissions: ["http://localhost/*"]` (covers any port on localhost)
- `commands` section defines `Alt+Shift+B` shortcut
- Service worker (background.js) handles all logic

**CORS note:** The extension makes requests from its own origin (`chrome-extension://...`), which browsers treat differently from page-context JS. Manifest V3 extensions with `host_permissions` bypass CORS entirely, so no server-side CORS changes needed for the extension. The bookmarklet, however, runs in the page's origin and will need CORS headers (see below).

### Hotkey Script

```bash
#!/usr/bin/env bash
# obsidian-borg-capture.sh — capture clipboard URL to obsidian-borg

# Cross-platform clipboard read
case "$(uname)" in
    Darwin) URL="$(pbpaste)" ;;
    Linux)
        if command -v wl-paste &>/dev/null && [[ -n "$WAYLAND_DISPLAY" ]]; then
            URL="$(wl-paste 2>/dev/null)"
        else
            URL="$(xclip -selection clipboard -o 2>/dev/null)"
        fi
        ;;
    *) echo "Unsupported OS" >&2; exit 1 ;;
esac

if [[ -z "$URL" || ! "$URL" =~ ^https?:// ]]; then
    notify_msg "No URL found on clipboard"
    exit 1
fi

RESULT=$(obsidian-borg ingest "$URL" 2>&1)
EXIT_CODE=$?

# Cross-platform notification
notify_msg() {
    case "$(uname)" in
        Darwin) osascript -e "display notification \"$1\" with title \"obsidian-borg\"" ;;
        Linux)  notify-send "obsidian-borg" "$1" --urgency="${2:-low}" ;;
    esac
}

if [[ $EXIT_CODE -eq 0 ]]; then
    notify_msg "Captured: $RESULT"
else
    notify_msg "Failed: $RESULT" "critical"
fi
```

Requires `obsidian-borg` on `$PATH`.

**Platform dependencies:**

| Platform | Clipboard | Notifications |
|----------|-----------|---------------|
| macOS | `pbpaste` (built-in) | `osascript` (built-in) |
| Linux (X11) | `xclip` | `notify-send` (libnotify) |
| Linux (Wayland) | `wl-paste` (wl-clipboard) | `notify-send` (libnotify) |

Workflow: copy URL in browser -> hit global hotkey -> `notify-send` confirms.

README covers binding to a keyboard shortcut in:
- **GNOME:** Settings > Keyboard > Custom Shortcuts
- **KDE:** System Settings > Shortcuts > Custom Shortcuts
- **i3/sway:** `bindsym` in config
- **macOS:** System Settings > Keyboard > Keyboard Shortcuts > App Shortcuts, or Automator Quick Action + shortcut, or Hammerspoon `hs.hotkey.bind`

### File Layout

```
clients/
  bookmarklet/
    README.md
  extension/
    manifest.json
    background.js
    popup.html
    popup.js
    options.html
    options.js
    icons/
      icon-16.png
      icon-48.png
      icon-128.png
  hotkey/
    obsidian-borg-capture.sh
    README.md
```

### Implementation Plan

**Phase 1: Unified secret resolution**
- Add `shellexpand` dependency
- Implement `resolve_secret()` helper
- Rename config fields: `bot_token_env` -> `bot_token`, `api_key_env` -> `api_key`
- Update all call sites that read secrets
- Tests for file-based and env-var-based resolution

**Phase 2: CLI subcommands**
- Refactor `src/cli.rs` to use clap subcommands (`serve`, `ingest`, `install`, `uninstall`)
- Add `run_ingest()` to `src/lib.rs`
- Update `src/main.rs` dispatch
- `None | Serve` = existing daemon behavior (backward compat)
- Tests for CLI parsing and ingest subcommand

**Phase 3: Install / uninstall**
- Implement `install_service()` — generates hardened systemd user unit (Linux) or launchd plist (macOS)
- Implement `uninstall_service()` — stops, disables, removes
- Replaces static `deploy/obsidian-borg.service` with dynamically generated unit

**Phase 4: CORS support** (required only for bookmarklet; extension and hotkey/CLI work without it)
- Add CORS middleware to the axum router (tower-http `CorsLayer`)
- Allow `Origin: *` on `/ingest` and `/health` (localhost-only service)
- Handles `OPTIONS` preflight automatically

**Phase 5: Client artifacts**
- Bookmarklet README
- Hotkey script + README
- WebExtension (manifest, background, popup, options, icons)

**Phase 6: Testing**
- Manual test each client against running daemon
- Unit tests for CLI subcommand parsing, secret resolution
- Integration test for `run_ingest()` hitting a mock server
- Test `install`/`uninstall` cycle on Linux

## Alternatives Considered

### Alternative 1: Separate CLI binary via Cargo workspace

- **Description:** Split into `obsidian-borg-daemon` and `obsidian-borg-cli` binaries in a workspace.
- **Pros:** Clean separation of concerns.
- **Cons:** Already collapsed the workspace once. Two binaries to install/deploy. Shared config loading duplicated.
- **Why not chosen:** Single binary with subcommands is simpler and the project already moved away from workspaces.

### Alternative 2: brotab for URL extraction in hotkey script

- **Description:** Use `brotab` to query the active browser tab's URL directly.
- **Pros:** Gets the exact URL without relying on clipboard.
- **Cons:** Requires its own browser extension installed. Another dependency. Only works with browsers that have the brotab extension.
- **Why not chosen:** Clipboard-based approach is universal, zero-dependency (just `xclip`), and works with any application.

### Alternative 3: xdotool Ctrl+L/Ctrl+C approach

- **Description:** Use `xdotool` to simulate Ctrl+L (select address bar), Ctrl+C (copy), then read clipboard.
- **Pros:** Grabs URL from browser without manual copy step.
- **Cons:** Fragile — breaks if focus shifts, doesn't work in Wayland, assumes browser is focused, adds race conditions.
- **Why not chosen:** Too brittle. Clipboard-based is one extra step (Ctrl+C) but reliable.

### Alternative 4: Native messaging from extension to read yml config

- **Description:** Use WebExtension native messaging to spawn a local process that reads obsidian-borg.yml and returns host/port.
- **Pros:** Extension always has correct config, single source of truth.
- **Cons:** Complex setup (native messaging host manifest, platform-specific install). Over-engineered for a host:port value.
- **Why not chosen:** Auto-discovery via `/health` achieves the same goal with zero install complexity.

## Technical Considerations

### Dependencies

**New Rust dependencies:**
- `tower-http` (for CORS middleware) — already using `tower`
- `shellexpand` (for `~` expansion in secret file paths)

**Client-side:** No build tools needed. The extension is vanilla JS. The hotkey script needs clipboard and notification tools — on Linux: `xclip`/`wl-clipboard` + `notify-send`; on macOS: nothing extra (built-in `pbpaste` + `osascript`).

### CORS

The bookmarklet runs JavaScript in the context of the page you're viewing (e.g., `youtube.com`). The browser will enforce CORS on the `fetch()` to `localhost:8181`. We need the daemon to return:

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Methods: POST, GET
Access-Control-Allow-Headers: Content-Type
```

The `CorsLayer` from tower-http also handles the `OPTIONS` preflight request automatically, which the browser sends before the actual `POST`.

This is safe because the daemon only listens on localhost. The extension does NOT need CORS (Manifest V3 host_permissions bypass it), but having it doesn't hurt.

### Cross-Platform Support (Linux + macOS)

**What's already cross-platform (no changes needed):**

| Component | Why it works |
|-----------|-------------|
| Rust daemon | Cross-platform by nature; compiles on both |
| CLI `ingest` subcommand | Same — pure Rust + reqwest |
| Config resolution | Uses `dirs::config_dir()` which resolves to `~/.config/` (Linux) or `~/Library/Application Support/` (macOS) |
| Bookmarklet | Pure browser JS, OS-agnostic |
| WebExtension | Manifest V3 is identical on Chrome/Firefox regardless of OS |

**What needs OS awareness:**

| Component | Linux | macOS |
|-----------|-------|-------|
| Hotkey script: clipboard | `xclip` / `wl-paste` | `pbpaste` (built-in) |
| Hotkey script: notifications | `notify-send` (libnotify) | `osascript` (built-in) |
| Hotkey binding setup | GNOME/KDE/i3 settings | System Settings, Automator, or Hammerspoon |
| Daemon service | `systemd` unit (`deploy/obsidian-borg.service`) | `launchd` plist (future: `deploy/macos/com.obsidian-borg.plist`) |

The hotkey script handles this via `uname` detection. macOS has zero external dependencies (`pbpaste` and `osascript` are built-in), making it simpler than Linux.

**Out of scope for this design:** A macOS launchd plist for running the daemon as a service. The existing `deploy/` directory has a systemd unit; a macOS equivalent is a natural follow-up but not part of this work.

### Performance

No impact on the daemon. The CLI `ingest` subcommand is a single HTTP request — sub-second cold start since it's a compiled Rust binary.

### Security

- Daemon listens on localhost only (`0.0.0.0` in current config should be reviewed — CORS + `0.0.0.0` means any device on the network can POST)
- Bookmarklet: runs in page context, so malicious pages could theoretically observe the fetch. Low risk since it only sends the current page URL to localhost.
- Extension: isolated context, no page-level exposure.
- Hotkey script: reads clipboard, which could contain non-URL content. Validated with `^https?://` regex before sending.
- **Secrets:** File-based resolution reads `~/.config/<company>/api-token` with `0600` permissions (user-only). No secrets in systemd unit files, env vars, or command-line arguments. The `ProtectHome=read-only` hardening directive ensures the daemon can read but not modify secret files.
- **systemd hardening:** User unit with `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=read-only`, `ReadWritePaths` limited to the vault inbox. The daemon runs as the user, not root.

### Testing Strategy

- **CLI subcommand parsing:** Unit tests in `src/cli.rs`
- **`run_ingest` function:** Integration test with a mock axum server
- **Bookmarklet:** Manual test in browser
- **Extension:** Manual load as unpacked extension, test click + shortcut
- **Hotkey script:** Manual test with `xclip` and `notify-send`

### Rollout Plan

1. Merge secret resolution + CLI subcommands (Rust code)
2. Merge install/uninstall + CORS (Rust code)
3. Add client artifacts (no Rust changes)
4. Document in project README
5. `obsidian-borg install` on the Ubuntu machine to switch from manual systemd setup

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| CORS blocks bookmarklet on some browsers | Medium | Medium | Test across Chrome/Firefox; extension is the primary desktop client |
| Mixed content (HTTPS page -> HTTP localhost) blocks bookmarklet | Medium | Medium | Chrome allows `http://localhost` from HTTPS as a special case; Firefox may block. Extension is unaffected. Document as known bookmarklet limitation. |
| `xclip` not available on Wayland | Medium | Low | Script auto-detects Wayland and uses `wl-paste`; documented in README |
| macOS keybinding setup more complex | Low | Low | README documents Automator and Hammerspoon approaches |
| Extension auto-discovery fails (daemon not running) | Medium | Low | Fallback to options page; clear error message |
| `0.0.0.0` bind + CORS allows LAN access | Low | Medium | Document; consider defaulting to `127.0.0.1` |

## Open Questions

- [ ] Should the default bind address change from `0.0.0.0` to `127.0.0.1` now that CORS is being added?
- [ ] Extension icon — generate simple placeholder PNGs or use an existing obsidian-borg logo?
- [ ] Should the bookmarklet show an `alert()` or inject a brief toast notification into the page?

## References

- [obsidian-ingestion-ideas.md](../obsidian-ingestion-ideas.md) — original architecture brainstorm
- [Chrome Extensions Manifest V3 docs](https://developer.chrome.com/docs/extensions/mv3/)
- [tower-http CORS](https://docs.rs/tower-http/latest/tower_http/cors/)
- [WebExtension commands API](https://developer.mozilla.org/en-US/docs/Mozilla/Add-ons/WebExtensions/API/commands)
