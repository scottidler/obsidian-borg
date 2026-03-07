#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

pub mod cli;
pub mod config;
pub mod discord;
pub mod error;
pub mod health;
pub mod jina;
pub mod logging;
pub mod markdown;
pub mod pipeline;
pub mod routes;
pub mod telegram;
pub mod transcription_client;
pub mod types;
pub mod url_hygiene;
pub mod url_router;
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
    log::debug!(
        "Groq model: {}, key env: {}",
        config.groq.model,
        config.groq.api_key_env
    );
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
        let token = std::env::var(&tg_config.bot_token_env).context("Telegram bot token env var not set")?;
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
        let token = std::env::var(&dc_config.bot_token_env).context("Discord bot token env var not set")?;
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
