#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

use axum::Router;
use axum::routing::{get, post};
use clap::Parser;
use colored::*;
use eyre::{Context, Result};
use log::info;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

mod cli;
mod config;
mod jina;
mod markdown;
mod pipeline;
mod routes;
mod transcription_client;
mod url_router;
mod youtube;

use cli::Cli;
use config::Config;

fn build_router(config: Arc<Config>) -> Router {
    Router::new()
        .route("/health", get(routes::health))
        .route("/ingest", post(routes::ingest))
        .with_state(config)
}

async fn run_server(config: Config) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .context("Invalid server address")?;

    let config = Arc::new(config);
    let app = build_router(config);

    println!("{} borg-daemon listening on {}", "-->".green(), addr.to_string().cyan());

    let listener = TcpListener::bind(addr).await.context("Failed to bind to address")?;

    axum::serve(listener, app).await.context("Server error")?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    borg_core::setup_logging("borg-daemon").context("Failed to setup logging")?;

    let cli = Cli::parse();

    let config: Config =
        borg_core::load_config("borg-daemon", cli.config.as_ref()).context("Failed to load configuration")?;

    info!("Starting borg-daemon with config from: {:?}", cli.config);

    if cli.verbose {
        println!("{}", "Verbose mode enabled".yellow());
    }

    run_server(config).await
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
