use axum::Json;
use axum::extract::State;
use std::sync::Arc;

use crate::config::Config;
use crate::health::HealthResponse;
use crate::pipeline;
use crate::types::{IngestMethod, IngestRequest, IngestResult, IngestStatus};

pub async fn health() -> Json<HealthResponse> {
    crate::health::health_handler("obsidian-borg", env!("GIT_DESCRIBE")).await
}

pub async fn ingest(State(config): State<Arc<Config>>, Json(request): Json<IngestRequest>) -> Json<IngestResult> {
    log::info!("Received ingest request for URL: {}", request.url);

    let tags = request.tags.unwrap_or_default();

    let method = request.method.unwrap_or(IngestMethod::Http);
    let result = pipeline::process_url(&request.url, tags, method, request.force, &config).await;

    match &result.status {
        IngestStatus::Failed { reason } => {
            log::warn!("Ingest failed for {}: {reason}", request.url);
        }
        IngestStatus::Completed => {
            log::info!("Ingest completed for {}", request.url);
        }
        IngestStatus::Duplicate { .. } => {
            log::info!("Duplicate URL skipped for {}", request.url);
        }
        IngestStatus::Queued => {}
    }

    Json(result)
}
