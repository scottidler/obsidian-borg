use crate::config::Config;
use crate::jina;
use crate::markdown::{self, ContentType, NoteContent};
use crate::transcription_client::TranscriptionClient;
use crate::types::{AudioFormat, IngestResult, IngestStatus};
use crate::url_router::{self, UrlType};
use crate::youtube;
use eyre::{Context, Result};
use std::path::PathBuf;

pub async fn process_url(url: &str, tags: Vec<String>, config: &Config) -> IngestResult {
    match process_url_inner(url, tags, config).await {
        Ok(result) => result,
        Err(e) => {
            log::error!("Pipeline failed for {url}: {e}");
            IngestResult {
                status: IngestStatus::Failed { reason: format!("{e}") },
                note_path: None,
                title: None,
                tags: vec![],
            }
        }
    }
}

async fn process_url_inner(url: &str, tags: Vec<String>, config: &Config) -> Result<IngestResult> {
    let url_type = url_router::classify_url(url)?;

    let (title, summary, content_type) = match url_type {
        UrlType::YouTube(yt_url) => process_youtube(&yt_url, config).await?,
        UrlType::Article(article_url) => process_article(&article_url).await?,
    };

    let note = NoteContent {
        title: title.clone(),
        source_url: url.to_string(),
        tags: tags.clone(),
        summary,
        content_type,
    };

    let rendered = markdown::render_note(&note);
    let filename = format!("{}.md", markdown::sanitize_filename(&title));

    let inbox_path = expand_tilde(&config.vault.inbox_path);
    std::fs::create_dir_all(&inbox_path).context("Failed to create vault inbox directory")?;

    let note_path = inbox_path.join(&filename);
    std::fs::write(&note_path, &rendered).context("Failed to write note to vault")?;

    log::info!("Wrote note: {}", note_path.display());

    Ok(IngestResult {
        status: IngestStatus::Completed,
        note_path: Some(note_path.to_string_lossy().to_string()),
        title: Some(title),
        tags,
    })
}

async fn process_youtube(url: &str, config: &Config) -> Result<(String, String, ContentType)> {
    let metadata = youtube::fetch_metadata(url)?;

    // Tier 1: Try auto-subtitles
    let transcript = match youtube::fetch_subtitles(url)? {
        Some(subs) => {
            log::info!("Got auto-subtitles for: {}", metadata.title);
            subs
        }
        None => {
            log::info!("No auto-subs, extracting audio for: {}", metadata.title);
            // Extract audio and transcribe via Tier 2/3
            let temp_dir = std::env::temp_dir().join("obsidian-borg");
            std::fs::create_dir_all(&temp_dir)?;
            let audio_path = youtube::extract_audio(url, &temp_dir.to_string_lossy())?;
            let audio_bytes = std::fs::read(&audio_path)?;
            let _ = std::fs::remove_file(&audio_path);

            let groq_key = std::env::var(&config.groq.api_key_env).ok();
            let client = TranscriptionClient::new(
                &config.transcriber.url,
                groq_key,
                &config.groq.model,
                config.transcriber.timeout_secs,
            );

            let response = client.transcribe(audio_bytes, AudioFormat::Mp3, None).await?;
            response.text
        }
    };

    let content_type = ContentType::YouTube {
        uploader: metadata.uploader.clone(),
        duration_secs: metadata.duration_secs,
    };

    Ok((metadata.title, transcript, content_type))
}

async fn process_article(url: &str) -> Result<(String, String, ContentType)> {
    let article_md = jina::fetch_article_markdown(url).await?;

    let title = article_md
        .lines()
        .find(|line| line.starts_with("# "))
        .map(|line| line.trim_start_matches("# ").to_string())
        .unwrap_or_else(|| url.to_string());

    Ok((title, article_md, ContentType::Article))
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(stripped);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_tilde() {
        let expanded = expand_tilde("~/test/path");
        assert!(!expanded.to_string_lossy().starts_with("~/"));
        assert!(expanded.to_string_lossy().ends_with("test/path"));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let expanded = expand_tilde("/absolute/path");
        assert_eq!(expanded, PathBuf::from("/absolute/path"));
    }
}
