#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

use axum::Router;
use axum::routing::{get, post};
use clap::Parser;
use eyre::{Context, Result};
use log::info;
use std::net::SocketAddr;
use tokio::net::TcpListener;

mod cli;
mod config;
mod routes;

use cli::Cli;
use config::Config;

fn build_router() -> Router {
    Router::new()
        .route("/health", get(routes::health))
        .route("/transcribe", post(routes::transcribe))
}

async fn run_server(config: &Config) -> Result<()> {
    let addr: SocketAddr = format!("{}:{}", config.server.host, config.server.port)
        .parse()
        .context("Invalid server address")?;

    let app = build_router();

    println!("borg-transcriber listening on {addr}");

    let listener = TcpListener::bind(addr).await.context("Failed to bind to address")?;

    axum::serve(listener, app).await.context("Server error")?;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    borg_core::setup_logging("borg-transcriber").context("Failed to setup logging")?;

    let cli = Cli::parse();

    let config: Config =
        borg_core::load_config("borg-transcriber", cli.config.as_ref()).context("Failed to load configuration")?;

    info!("Starting borg-transcriber with config from: {:?}", cli.config);

    run_server(&config).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_endpoint() {
        let app = build_router();
        let req = Request::builder().uri("/health").body(Body::empty()).expect("request");
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_transcribe_endpoint_stub() {
        let app = build_router();
        let body = serde_json::json!({
            "audio_bytes": [1, 2, 3],
            "language": "en",
            "format": "Mp3"
        });
        let req = Request::builder()
            .method("POST")
            .uri("/transcribe")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).expect("json")))
            .expect("request");
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
