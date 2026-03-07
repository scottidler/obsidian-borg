#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod cli;
pub mod config;
pub mod discord;
pub mod error;
pub mod fabric;
pub mod health;
pub mod hygiene;
pub mod jina;
pub mod logging;
pub mod markdown;
pub mod pipeline;
pub mod router;
pub mod routes;
pub mod telegram;
pub mod transcription;
pub mod types;
pub mod youtube;

use axum::Router;
use axum::routing::{get, post};
use colored::*;
use eyre::{Context, Result};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

use config::Config;

pub fn build_router(config: Arc<Config>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    Router::new()
        .route("/health", get(routes::health))
        .route("/ingest", post(routes::ingest))
        .layer(cors)
        .with_state(config)
}

pub async fn run_server(config: Config, _verbose: bool) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .context("Invalid server address")?;

    log::info!("Server address: {addr}");
    log::debug!("Vault inbox: {}", config.vault.inbox_path);
    log::debug!("Transcriber URL: {}", config.transcriber.url);
    log::debug!("Groq model: {}", config.groq.model);
    log::debug!("LLM provider: {}, model: {}", config.llm.provider, config.llm.model);

    let config = Arc::new(config);
    let mut tasks = tokio::task::JoinSet::new();

    // HTTP server (always runs)
    let app = build_router(config.clone());
    let listener = TcpListener::bind(addr).await.context("Failed to bind to address")?;
    tasks.spawn(async move { axum::serve(listener, app).await.map_err(|e| eyre::eyre!(e)) });
    log::info!("HTTP server listening on {addr}");
    println!("{} http server on {}", "-->".green(), addr.to_string().cyan());

    // Telegram bot (config-driven)
    if let Some(tg_config) = &config.telegram {
        let token = config::resolve_secret(&tg_config.bot_token).context("Failed to resolve Telegram bot token")?;
        log::info!(
            "Telegram bot enabled (allowed_chat_ids: {:?})",
            tg_config.allowed_chat_ids
        );
        let tg = tg_config.clone();
        let cfg = config.clone();
        tasks.spawn(async move { telegram::run(token, tg, cfg).await });
        println!("{} telegram bot active", "-->".green());
    }

    // Discord bot (config-driven)
    if let Some(dc_config) = &config.discord {
        let token = config::resolve_secret(&dc_config.bot_token).context("Failed to resolve Discord bot token")?;
        log::info!("Discord bot enabled (channel_id: {})", dc_config.channel_id);
        let dc = dc_config.clone();
        let cfg = config.clone();
        tasks.spawn(async move { discord::run(token, dc, cfg).await });
        println!("{} discord bot active", "-->".green());
    }

    // If any task exits (error or completion), propagate
    if let Some(result) = tasks.join_next().await {
        result??;
    }

    Ok(())
}

pub async fn run_ingest(config: Config, url: String, tags: Option<Vec<String>>) -> Result<()> {
    let host = &config.server.host;
    let port = config.server.port;
    let endpoint = format!("http://{host}:{port}/ingest");

    let body = serde_json::json!({
        "url": url,
        "tags": tags.unwrap_or_default(),
    });

    let client = reqwest::Client::new();
    let response = client.post(&endpoint).json(&body).send().await.map_err(|e| {
        if e.is_connect() {
            eyre::eyre!("cannot reach obsidian-borg at http://{host}:{port} — is the daemon running?")
        } else {
            eyre::eyre!("{e}")
        }
    })?;

    let result: types::IngestResult = response.json().await.context("Failed to parse response from daemon")?;

    match &result.status {
        types::IngestStatus::Completed => {
            let title = result.title.as_deref().unwrap_or("Untitled");
            let path = result.note_path.as_deref().unwrap_or("unknown");
            println!("Captured: \"{title}\" -> {path}");
        }
        types::IngestStatus::Failed { reason } => {
            eprintln!("Error: {reason}");
            std::process::exit(1);
        }
        types::IngestStatus::Queued => {
            println!("Queued for processing.");
        }
    }

    Ok(())
}

pub async fn install_service(force: bool) -> Result<()> {
    let exe_path = std::env::current_exe().context("Failed to detect binary path")?;
    let exe = exe_path.display();

    if cfg!(target_os = "linux") {
        install_systemd(&exe.to_string(), force).await
    } else if cfg!(target_os = "macos") {
        install_launchd(&exe.to_string(), force).await
    } else {
        eyre::bail!("Unsupported platform for service install")
    }
}

pub async fn uninstall_service() -> Result<()> {
    if cfg!(target_os = "linux") {
        uninstall_systemd().await
    } else if cfg!(target_os = "macos") {
        uninstall_launchd().await
    } else {
        eyre::bail!("Unsupported platform for service uninstall")
    }
}

async fn install_systemd(exe_path: &str, force: bool) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| eyre::eyre!("Cannot determine home directory"))?;
    let unit_dir = home.join(".config/systemd/user");
    let unit_path = unit_dir.join("obsidian-borg.service");

    if unit_path.exists() && !force {
        eyre::bail!(
            "Service file already exists at {}. Use --force to overwrite.",
            unit_path.display()
        );
    }

    let vault_path = home.join("repos/scottidler/obsidian");
    let unit_content = format!(
        r#"[Unit]
Description=obsidian-borg - Obsidian ingestion daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={exe_path} serve
Restart=always
RestartSec=5
StartLimitBurst=5
StartLimitIntervalSec=60
WorkingDirectory={home}

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths={vault}
PrivateTmp=true

[Install]
WantedBy=default.target
"#,
        home = home.display(),
        vault = vault_path.display(),
    );

    std::fs::create_dir_all(&unit_dir).context("Failed to create systemd user unit directory")?;
    std::fs::write(&unit_path, &unit_content).context("Failed to write systemd unit file")?;
    println!("Wrote {}", unit_path.display());

    let status = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("Failed to run systemctl daemon-reload")?;
    if !status.success() {
        eyre::bail!("systemctl --user daemon-reload failed");
    }

    let status = std::process::Command::new("systemctl")
        .args(["--user", "enable", "--now", "obsidian-borg"])
        .status()
        .context("Failed to enable obsidian-borg service")?;
    if !status.success() {
        eyre::bail!("systemctl --user enable --now obsidian-borg failed");
    }

    println!("Service installed and started.");
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "status", "obsidian-borg", "--no-pager"])
        .status();

    Ok(())
}

async fn install_launchd(exe_path: &str, force: bool) -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| eyre::eyre!("Cannot determine home directory"))?;
    let plist_dir = home.join("Library/LaunchAgents");
    let plist_path = plist_dir.join("com.obsidian-borg.plist");

    if plist_path.exists() && !force {
        eyre::bail!(
            "Plist already exists at {}. Use --force to overwrite.",
            plist_path.display()
        );
    }

    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.obsidian-borg</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe_path}</string>
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
"#
    );

    std::fs::create_dir_all(&plist_dir).context("Failed to create LaunchAgents directory")?;
    std::fs::write(&plist_path, &plist_content).context("Failed to write plist file")?;
    println!("Wrote {}", plist_path.display());

    let status = std::process::Command::new("launchctl")
        .args(["load", &plist_path.to_string_lossy()])
        .status()
        .context("Failed to run launchctl load")?;
    if !status.success() {
        eyre::bail!("launchctl load failed");
    }

    println!("Service installed and started.");
    Ok(())
}

async fn uninstall_systemd() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| eyre::eyre!("Cannot determine home directory"))?;
    let unit_path = home.join(".config/systemd/user/obsidian-borg.service");

    if !unit_path.exists() {
        println!("No service file found at {}", unit_path.display());
        return Ok(());
    }

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", "--now", "obsidian-borg"])
        .status();

    std::fs::remove_file(&unit_path).context("Failed to remove unit file")?;
    println!("Removed {}", unit_path.display());

    let _ = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status();

    println!("Service uninstalled.");
    Ok(())
}

async fn uninstall_launchd() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| eyre::eyre!("Cannot determine home directory"))?;
    let plist_path = home.join("Library/LaunchAgents/com.obsidian-borg.plist");

    if !plist_path.exists() {
        println!("No plist found at {}", plist_path.display());
        return Ok(());
    }

    let _ = std::process::Command::new("launchctl")
        .args(["unload", &plist_path.to_string_lossy()])
        .status();

    std::fs::remove_file(&plist_path).context("Failed to remove plist file")?;
    println!("Removed {}", plist_path.display());
    println!("Service uninstalled.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_router() -> Router {
        build_router(Arc::new(Config::default()))
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = test_router();
        let req = Request::builder().uri("/health").body(Body::empty()).expect("request");
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_ingest_endpoint() {
        let app = test_router();
        let body = serde_json::json!({"url": "https://youtube.com/watch?v=test"});
        let req = Request::builder()
            .method("POST")
            .uri("/ingest")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).expect("json")))
            .expect("request");
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
