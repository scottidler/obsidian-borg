use axum::Json;
use borg_core::HealthResponse;
use borg_core::types::{TranscriptionRequest, TranscriptionResponse};

pub async fn health() -> Json<HealthResponse> {
    borg_core::health_handler("borg-transcriber", env!("GIT_DESCRIBE")).await
}

pub async fn transcribe(Json(request): Json<TranscriptionRequest>) -> Json<TranscriptionResponse> {
    log::info!(
        "Received transcription request: {} bytes, format: {:?}",
        request.audio_bytes.len(),
        request.format
    );

    Json(TranscriptionResponse {
        text: "Transcription stub - not yet implemented".to_string(),
        language: request.language.unwrap_or_else(|| "en".to_string()),
        duration_secs: 0.0,
    })
}
