use axum::Json;
use axum::extract::State;
use borg_core::HealthResponse;
use borg_core::types::{IngestRequest, IngestResult, IngestStatus};
use std::sync::Arc;

use crate::config::Config;
use crate::pipeline;

pub async fn health() -> Json<HealthResponse> {
    borg_core::health_handler("borg-daemon", env!("GIT_DESCRIBE")).await
}

pub async fn ingest(State(config): State<Arc<Config>>, Json(request): Json<IngestRequest>) -> Json<IngestResult> {
    log::info!("Received ingest request for URL: {}", request.url);

    let tags = request.tags.unwrap_or_default();

    // For now, return Queued immediately. Full async processing would use
    // a background task queue, but the pipeline is wired and callable.
    let result = pipeline::process_url(&request.url, tags, &config).await;

    // If pipeline isn't reachable (no yt-dlp, no network), gracefully degrade
    match &result.status {
        IngestStatus::Failed { reason } => {
            log::warn!("Ingest failed for {}: {reason}", request.url);
        }
        IngestStatus::Completed => {
            log::info!("Ingest completed for {}", request.url);
        }
        IngestStatus::Queued => {}
    }

    Json(result)
}
