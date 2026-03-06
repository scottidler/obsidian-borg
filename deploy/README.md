# Deployment Guide

## Architecture

```
Phone/Desktop --HTTP POST-----> obsidian-borg (Ubuntu desktop)
Telegram DM --long poll-------> obsidian-borg
Discord channel --websocket---> obsidian-borg
obsidian-borg --HTTP POST-----> youtube-siphon (WSL2/4090, when needed)
              --HTTP POST-----> Groq API (when youtube-siphon unreachable)
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

Create `~/.config/obsidian-borg/obsidian-borg.yml` (see `obsidian-borg.example.yml` at repo root):

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

## Telegram Bot (Optional)

1. Message [@BotFather](https://t.me/BotFather) on Telegram
2. Send `/newbot`, follow prompts to create a bot
3. Copy the bot token
4. Add to your systemd service or shell environment:
   ```bash
   Environment=TELEGRAM_BOT_TOKEN=your-token-here
   ```
5. Add the telegram section to your config:
   ```yaml
   telegram:
     bot_token_env: "TELEGRAM_BOT_TOKEN"
     allowed_chat_ids: []  # empty = allow all
   ```
6. Restart the service. Share any URL to your bot's DM and it will be ingested.

To restrict access, find your chat ID by sending a message to the bot and checking the logs, then add it to `allowed_chat_ids`.

## Discord Bot (Optional)

1. Go to the [Discord Developer Portal](https://discord.com/developers/applications)
2. Create a new application, then add a Bot
3. Enable the **MESSAGE CONTENT** privileged intent under Bot settings
4. Copy the bot token
5. Invite the bot to your server with the OAuth2 URL Generator (scopes: `bot`; permissions: `Send Messages`, `Read Message History`)
6. Get the channel ID (enable Developer Mode in Discord settings, right-click the channel, Copy ID)
7. Add to your systemd service or shell environment:
   ```bash
   Environment=DISCORD_BOT_TOKEN=your-token-here
   ```
8. Add the discord section to your config:
   ```yaml
   discord:
     bot_token_env: "DISCORD_BOT_TOKEN"
     channel_id: 1234567890  # your channel ID
   ```
9. Restart the service. Post any URL in the designated channel and it will be ingested.

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
