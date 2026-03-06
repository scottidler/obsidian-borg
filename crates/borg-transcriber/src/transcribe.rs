use borg_core::types::{TranscriptionRequest, TranscriptionResponse};
use eyre::Result;
use std::sync::Arc;

pub trait Transcriber: Send + Sync {
    fn transcribe(&self, request: &TranscriptionRequest) -> Result<TranscriptionResponse>;
    fn name(&self) -> &str;
}

pub struct StubTranscriber;

impl Transcriber for StubTranscriber {
    fn transcribe(&self, request: &TranscriptionRequest) -> Result<TranscriptionResponse> {
        log::info!(
            "Stub transcriber: {} bytes, format: {:?}",
            request.audio_bytes.len(),
            request.format
        );
        Ok(TranscriptionResponse {
            text: "Stub transcription - whisper-rs feature not enabled".to_string(),
            language: request.language.clone().unwrap_or_else(|| "en".to_string()),
            duration_secs: 0.0,
        })
    }

    fn name(&self) -> &str {
        "stub"
    }
}

pub type SharedTranscriber = Arc<dyn Transcriber>;

pub fn create_transcriber() -> SharedTranscriber {
    log::info!("Using stub transcriber (whisper-rs feature not enabled)");
    Arc::new(StubTranscriber)
}

#[cfg(test)]
mod tests {
    use super::*;
    use borg_core::types::AudioFormat;

    #[test]
    fn test_stub_transcriber() {
        let transcriber = StubTranscriber;
        let request = TranscriptionRequest {
            audio_bytes: vec![1, 2, 3, 4],
            language: Some("en".to_string()),
            format: AudioFormat::Mp3,
        };
        let response = transcriber.transcribe(&request).expect("should succeed");
        assert_eq!(response.language, "en");
        assert!(response.text.contains("Stub"));
    }

    #[test]
    fn test_stub_transcriber_default_language() {
        let transcriber = StubTranscriber;
        let request = TranscriptionRequest {
            audio_bytes: vec![1, 2, 3],
            language: None,
            format: AudioFormat::Wav,
        };
        let response = transcriber.transcribe(&request).expect("should succeed");
        assert_eq!(response.language, "en");
    }

    #[test]
    fn test_transcriber_name() {
        let transcriber = StubTranscriber;
        assert_eq!(transcriber.name(), "stub");
    }

    #[test]
    fn test_create_transcriber_returns_stub() {
        let transcriber = create_transcriber();
        assert_eq!(transcriber.name(), "stub");
    }
}
