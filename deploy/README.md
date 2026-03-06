# Deployment Guide

## Architecture

```
Phone/Desktop --HTTP POST--> borg-daemon (Ubuntu desktop)
borg-daemon --HTTP POST--> borg-transcriber (WSL2/4090, when needed)
            --HTTP POST--> Groq API (when transcriber unreachable)
```

Both machines must be on the same Tailscale network.

## Prerequisites

- [Tailscale](https://tailscale.com/) installed on both machines
- `yt-dlp` installed on the Ubuntu desktop: `sudo apt install yt-dlp`
- Whisper model files on the WSL2 machine (for borg-transcriber with whisper-rs feature)

## Ubuntu Desktop (borg-daemon)

### Build and install

```bash
cargo install --path crates/borg-daemon
```

### Configure

Create `~/.config/borg-daemon/borg-daemon.yml`:

```yaml
server:
  host: "0.0.0.0"
  port: 8080

vault:
  inbox_path: "~/obsidian-vault/Inbox"

transcriber:
  url: "http://100.x.x.x:8090"  # Tailscale IP of WSL2 machine
  timeout_secs: 120

groq:
  api_key_env: "GROQ_API_KEY"
  model: "whisper-large-v3"

llm:
  provider: "claude"
  model: "claude-sonnet-4-6"
  api_key_env: "ANTHROPIC_API_KEY"
```

### Install systemd service

```bash
sudo cp deploy/borg-daemon.service /etc/systemd/system/
# Edit the service file to set API keys
sudo systemctl daemon-reload
sudo systemctl enable --now borg-daemon
sudo systemctl status borg-daemon
```

## WSL2 Machine (borg-transcriber)

### Build and install

```bash
# With Whisper/CUDA support:
cargo install --path crates/borg-transcriber --features whisper-rs,cuda

# Without Whisper (stub mode for testing):
cargo install --path crates/borg-transcriber
```

### Configure

Create `~/.config/borg-transcriber/borg-transcriber.yml`:

```yaml
server:
  host: "0.0.0.0"
  port: 8090

whisper:
  model: "large-v3"
  model_path: "~/.local/share/whisper/models"
  device: "cuda"
  compute_type: "float16"
```

### Install systemd service

```bash
sudo cp deploy/borg-transcriber.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now borg-transcriber
sudo systemctl status borg-transcriber
```

## Android (HTTP Shortcuts)

Install [HTTP Shortcuts](https://http-shortcuts.rmy.ch/) on your Android device.

Create a shortcut:
- **Method:** POST
- **URL:** `http://100.x.x.x:8080/ingest` (Tailscale IP of Ubuntu desktop)
- **Content-Type:** application/json
- **Body:**
  ```json
  {"url": "{url}", "tags": ["shared"]}
  ```

Set this as a Share target so you can share URLs from any app directly to borg-daemon.

## Ubuntu Desktop Hotkey

Create a script at `~/.local/bin/borg-ingest`:

```bash
#!/bin/bash
URL=$(xclip -selection clipboard -o)
curl -s -X POST http://localhost:8080/ingest \
  -H "Content-Type: application/json" \
  -d "{\"url\": \"$URL\"}" | jq .
```

Bind to a keyboard shortcut in your desktop environment.

## Testing the Full Flow

```bash
# Health check
curl http://localhost:8080/health

# Ingest a YouTube video
curl -X POST http://localhost:8080/ingest \
  -H "Content-Type: application/json" \
  -d '{"url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ", "tags": ["test"]}'

# Ingest an article
curl -X POST http://localhost:8080/ingest \
  -H "Content-Type: application/json" \
  -d '{"url": "https://blog.rust-lang.org/2024/02/08/Rust-1.76.0.html", "tags": ["rust"]}'
```
