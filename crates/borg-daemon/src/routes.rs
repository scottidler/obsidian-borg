use axum::Json;
use borg_core::HealthResponse;
use borg_core::types::{IngestRequest, IngestResult, IngestStatus};

pub async fn health() -> Json<HealthResponse> {
    borg_core::health_handler("borg-daemon", env!("GIT_DESCRIBE")).await
}

pub async fn ingest(Json(request): Json<IngestRequest>) -> Json<IngestResult> {
    log::info!("Received ingest request for URL: {}", request.url);

    Json(IngestResult {
        status: IngestStatus::Queued,
        note_path: None,
        title: None,
        tags: request.tags.unwrap_or_default(),
    })
}
