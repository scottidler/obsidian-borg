use crate::config::Config;
use crate::fabric;
use crate::jina;
use crate::markdown::{self, ContentType, NoteContent};
use crate::transcription_client::TranscriptionClient;
use crate::types::{AudioFormat, IngestResult, IngestStatus};
use crate::url_hygiene;
use crate::url_router;
use crate::youtube;
use eyre::{Context, Result};
use std::path::PathBuf;
use std::time::Instant;

pub async fn process_url(url: &str, tags: Vec<String>, config: &Config) -> IngestResult {
    let start = Instant::now();
    match process_url_inner(url, tags, config).await {
        Ok(mut result) => {
            let elapsed = start.elapsed();
            log::info!("Pipeline completed for {url} in {elapsed:.2?}");
            result.elapsed_secs = Some(elapsed.as_secs_f64());
            result
        }
        Err(e) => {
            let elapsed = start.elapsed();
            log::error!("Pipeline failed for {url} in {elapsed:.2?}: {e:?}");
            let reason = format!("{:#}", e);
            IngestResult {
                status: IngestStatus::Failed { reason },
                note_path: None,
                title: None,
                tags: vec![],
                elapsed_secs: Some(elapsed.as_secs_f64()),
                folder: None,
            }
        }
    }
}

async fn process_url_inner(url: &str, tags: Vec<String>, config: &Config) -> Result<IngestResult> {
    log::debug!("Processing URL: {url}");

    let url_match = url_router::classify_url(url, &config.links)?;
    log::debug!(
        "URL classified as: {} (cleaned: {})",
        url_match.link_name,
        url_match.url
    );

    let use_fabric = fabric::is_available(&config.fabric);
    if !use_fabric {
        log::warn!("Fabric binary not available, falling back to legacy pipeline");
    }

    let (title, summary, content_type) = if url_match.is_youtube_type() {
        if use_fabric {
            process_youtube_fabric(&url_match.url, config).await?
        } else {
            process_youtube_legacy(&url_match.url, config).await?
        }
    } else if use_fabric {
        match process_article_fabric(&url_match.url, config).await {
            Ok(result) => result,
            Err(e) => {
                log::warn!("Fabric article fetch failed: {e:#}, falling back to Jina");
                process_article_jina(&url_match.url).await?
            }
        }
    } else {
        process_article_jina(&url_match.url).await?
    };

    let sanitized_tags: Vec<String> = tags.iter().map(|t| url_hygiene::sanitize_tag(t)).collect();

    let note = NoteContent {
        title: title.clone(),
        source_url: url_match.url.clone(),
        tags: sanitized_tags.clone(),
        summary,
        content_type,
    };

    let rendered = markdown::render_note(&note);
    let filename = format!("{}.md", url_hygiene::sanitize_filename(&title));

    let inbox_path = expand_tilde(&config.vault.inbox_path);
    std::fs::create_dir_all(&inbox_path).context("Failed to create vault inbox directory")?;

    let note_path = inbox_path.join(&filename);
    std::fs::write(&note_path, &rendered).context("Failed to write note to vault")?;

    log::info!("Wrote note: {}", note_path.display());

    Ok(IngestResult {
        status: IngestStatus::Completed,
        note_path: Some(note_path.to_string_lossy().to_string()),
        title: Some(title),
        tags: sanitized_tags,
        elapsed_secs: None,
        folder: None,
    })
}

async fn process_youtube_fabric(url: &str, config: &Config) -> Result<(String, String, ContentType)> {
    let yt = fabric::fetch_youtube(url, &config.fabric).await?;

    let transcript = if yt.transcript.is_empty() {
        log::warn!("Fabric returned empty transcript, falling back to yt-dlp");
        youtube::fetch_subtitles(url).await?.unwrap_or_default()
    } else {
        yt.transcript
    };

    // Summarize via Fabric (graceful failure)
    let summary = fabric::summarize(&transcript, true, &config.fabric)
        .await
        .unwrap_or_else(|e| {
            log::warn!("Fabric summarization failed: {e:#}");
            transcript.clone()
        });

    let content_type = ContentType::YouTube {
        uploader: yt.channel,
        duration_secs: yt.duration_secs,
    };

    Ok((yt.title, summary, content_type))
}

async fn process_youtube_legacy(url: &str, config: &Config) -> Result<(String, String, ContentType)> {
    log::debug!("Fetching YouTube metadata for: {url}");
    let metadata = youtube::fetch_metadata(url)?;

    let transcript = match youtube::fetch_subtitles(url).await? {
        Some(subs) => subs,
        None => {
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

async fn process_article_fabric(url: &str, config: &Config) -> Result<(String, String, ContentType)> {
    let article_md = fabric::fetch_article(url, &config.fabric).await?;

    let title = article_md
        .lines()
        .find(|line| line.starts_with("# "))
        .map(|line| line.trim_start_matches("# ").to_string())
        .unwrap_or_else(|| url.to_string());

    // Summarize via Fabric (graceful failure)
    let summary = fabric::summarize(&article_md, false, &config.fabric)
        .await
        .unwrap_or_else(|e| {
            log::warn!("Fabric summarization failed: {e:#}");
            article_md.clone()
        });

    Ok((title, summary, ContentType::Article))
}

async fn process_article_jina(url: &str) -> Result<(String, String, ContentType)> {
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
