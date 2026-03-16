use axum::Json;
use axum::extract::State;
use std::sync::Arc;

use serde::Deserialize;

use crate::config::Config;
use crate::health::HealthResponse;
use crate::pipeline;
use crate::types::{ContentKind, IngestMethod, IngestRequest, IngestResult, IngestStatus};

#[derive(Debug, Deserialize)]
pub struct NoteRequest {
    pub text: String,
    pub tags: Option<Vec<String>>,
}

pub async fn health() -> Json<HealthResponse> {
    crate::health::health_handler("obsidian-borg", env!("GIT_DESCRIBE")).await
}

pub async fn ingest(State(config): State<Arc<Config>>, Json(request): Json<IngestRequest>) -> Json<IngestResult> {
    log::info!("Received ingest request for URL: {}", request.url);

    let tags = request.tags.unwrap_or_default();

    let method = request.method.unwrap_or(IngestMethod::Http);
    let content = ContentKind::Url(request.url.clone());
    let result = pipeline::process_content(content, tags, method, request.force, &config).await;

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

pub async fn note(State(config): State<Arc<Config>>, Json(request): Json<NoteRequest>) -> Json<IngestResult> {
    log::info!("Received note request: {} chars", request.text.len());

    let tags = request.tags.unwrap_or_default();
    let content = ContentKind::Text(request.text);
    let result = pipeline::process_content(content, tags, IngestMethod::Http, false, &config).await;

    match &result.status {
        IngestStatus::Failed { reason } => {
            log::warn!("Note capture failed: {reason}");
        }
        IngestStatus::Completed => {
            log::info!("Note captured: {:?}", result.title);
        }
        _ => {}
    }

    Json(result)
}
