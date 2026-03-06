# Deployment Guide

## Architecture

```
Phone/Desktop --HTTP POST--> obsidian-borg (Ubuntu desktop)
obsidian-borg --HTTP POST--> youtube-siphon (WSL2/4090, when needed)
            --HTTP POST--> Groq API (when youtube-siphon unreachable)
```

Both machines must be on the same Tailscale network.

## Prerequisites

- [Tailscale](https://tailscale.com/) installed on both machines
- `yt-dlp` installed on the Ubuntu desktop: `sudo apt install yt-dlp`

## Ubuntu Desktop (obsidian-borg)

### Build and install

```bash
cargo install --path .
```

### Configure

Create `~/.config/obsidian-borg/obsidian-borg.yml` (see `deploy/obsidian-borg.example.yml`):

```bash
mkdir -p ~/.config/obsidian-borg
cp deploy/obsidian-borg.example.yml ~/.config/obsidian-borg/obsidian-borg.yml
# Edit to set your Tailscale IPs, vault path, etc.
```

### Install systemd service

```bash
sudo cp deploy/obsidian-borg.service /etc/systemd/system/
# Edit the service file to set API keys
sudo systemctl daemon-reload
sudo systemctl enable --now obsidian-borg
sudo systemctl status obsidian-borg
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

Set this as a Share target so you can share URLs from any app directly to obsidian-borg.

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
