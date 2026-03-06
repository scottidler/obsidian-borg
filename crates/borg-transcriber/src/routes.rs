use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use borg_core::HealthResponse;
use borg_core::types::{TranscriptionRequest, TranscriptionResponse};

use crate::transcribe::SharedTranscriber;

pub async fn health() -> Json<HealthResponse> {
    borg_core::health_handler("borg-transcriber", env!("GIT_DESCRIBE")).await
}

pub async fn transcribe(
    State(transcriber): State<SharedTranscriber>,
    Json(request): Json<TranscriptionRequest>,
) -> Result<Json<TranscriptionResponse>, (StatusCode, String)> {
    log::info!(
        "Received transcription request: {} bytes, format: {:?}",
        request.audio_bytes.len(),
        request.format
    );

    match transcriber.transcribe(&request) {
        Ok(response) => Ok(Json(response)),
        Err(e) => {
            log::error!("Transcription failed: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, format!("Transcription failed: {e}")))
        }
    }
}
