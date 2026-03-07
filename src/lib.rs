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

use config::Config;

pub fn build_router(config: Arc<Config>) -> Router {
    Router::new()
        .route("/health", get(routes::health))
        .route("/ingest", post(routes::ingest))
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

pub async fn install_service(_force: bool) -> Result<()> {
    eyre::bail!("install subcommand not yet implemented (Phase 3)")
}

pub async fn uninstall_service() -> Result<()> {
    eyre::bail!("uninstall subcommand not yet implemented (Phase 3)")
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
