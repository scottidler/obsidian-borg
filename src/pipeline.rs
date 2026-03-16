use crate::config::Config;
use crate::fabric;
use crate::hygiene;
use crate::jina;
use crate::ledger::{self, LedgerEntry, LedgerStatus};
use crate::markdown::{self, ContentType, NoteContent};
use crate::router;
use crate::transcription::TranscriptionClient;
use crate::types::{AudioFormat, IngestMethod, IngestResult, IngestStatus};
use crate::youtube;
use eyre::{Context, Result};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::time::Instant;
use tokio::sync::Mutex;

/// Extract the best title from fabric's markdown output.
///
/// Strategy (in priority order):
/// 1. `Title:` metadata line (fabric always emits this first)
///    - For HTML pages this is the <title> tag (usually great)
///    - For PDFs this is often the filename - we clean it up
/// 2. First `# ` heading in the markdown body
/// 3. Derive from the URL path (last segment, cleaned up)
/// 4. Raw URL as last resort
fn extract_article_title(article_md: &str, url: &str) -> String {
    // Strategy 1: Parse Title: metadata line
    if let Some(title) = article_md
        .lines()
        .find(|line| line.starts_with("Title:"))
        .map(|line| line.trim_start_matches("Title:").trim().to_string())
        && !title.is_empty()
    {
        // If it looks like a filename (has a file extension), clean it up
        let cleaned = if title.contains('.')
            && title
                .rsplit('.')
                .next()
                .is_some_and(|ext| matches!(ext.to_lowercase().as_str(), "pdf" | "html" | "htm" | "txt" | "md"))
        {
            // Strip extension, replace hyphens/underscores with spaces
            let without_ext = title.rsplit_once('.').map(|(base, _)| base).unwrap_or(&title);
            without_ext.replace(['-', '_'], " ")
        } else {
            title
        };
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    // Strategy 2: First # heading in the body (after "Markdown Content:" if present)
    let body_start = article_md
        .find("Markdown Content:")
        .map(|pos| pos + "Markdown Content:".len())
        .unwrap_or(0);
    if let Some(title) = article_md[body_start..]
        .lines()
        .find(|line| line.starts_with("# "))
        .map(|line| line.trim_start_matches("# ").trim().to_string())
        && !title.is_empty()
    {
        return title;
    }

    // Strategy 3: Derive from URL path
    if let Some(segment) = url
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && *s != url)
    {
        let cleaned = segment
            .rsplit_once('.')
            .map(|(base, _)| base)
            .unwrap_or(segment)
            .replace(['-', '_'], " ");
        if !cleaned.is_empty() {
            return cleaned;
        }
    }

    // Strategy 4: raw URL
    url.to_string()
}

/// In-memory dedup guard to prevent concurrent processing of the same canonical URL.
/// The ledger file is the durable dedup index, but concurrent tasks can race past
/// the ledger check before either writes its ✅ entry. This guard serializes that.
static INFLIGHT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub async fn process_url(
    url: &str,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> IngestResult {
    let start = Instant::now();
    match process_url_inner(url, tags, method, force, config).await {
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

            // Best-effort log failure to Borg Ledger
            let canonical =
                hygiene::normalize_url(url, &config.canonicalization.rules).unwrap_or_else(|_| url.to_string());

            // Release inflight guard on failure
            INFLIGHT.lock().await.remove(&canonical);
            let tz: chrono_tz::Tz = config
                .frontmatter
                .timezone
                .parse()
                .unwrap_or(chrono_tz::America::Los_Angeles);
            let now = chrono::Utc::now().with_timezone(&tz);
            let _ = ledger::append_entry(
                &ledger::ledger_path(config),
                &LedgerEntry {
                    date: now.format("%Y-%m-%d").to_string(),
                    time: now.format("%H:%M").to_string(),
                    method,
                    status: LedgerStatus::Failed,
                    title: None,
                    source: canonical.clone(),
                    folder: None,
                },
            );

            IngestResult {
                status: IngestStatus::Failed { reason },
                note_path: None,
                title: None,
                tags: vec![],
                elapsed_secs: Some(elapsed.as_secs_f64()),
                folder: None,
                method: Some(method),
                canonical_url: Some(canonical),
            }
        }
    }
}

async fn process_url_inner(
    url: &str,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> Result<IngestResult> {
    log::debug!("Processing URL: {url}");

    // Normalize URL (clean + canonicalize) before classification
    let canonical = hygiene::normalize_url(url, &config.canonicalization.rules)?;
    log::debug!("Canonical URL: {canonical}");
    if canonical != url {
        log::info!("URL canonicalized: {url} -> {canonical}");
    }

    // Get timezone for log timestamps
    let tz: chrono_tz::Tz = config
        .frontmatter
        .timezone
        .parse()
        .unwrap_or(chrono_tz::America::Los_Angeles);
    let now = chrono::Utc::now().with_timezone(&tz);
    let log_date = now.format("%Y-%m-%d").to_string();
    let log_time = now.format("%H:%M").to_string();

    // Dedup check (skip if --force)
    let ledger_file = ledger::ledger_path(config);
    if !force {
        // Check in-memory inflight guard first (prevents concurrent race)
        {
            let mut inflight = INFLIGHT.lock().await;
            if inflight.contains(&canonical) {
                log::info!("Duplicate URL (inflight): {canonical}");
                ledger::append_entry(
                    &ledger_file,
                    &LedgerEntry {
                        date: log_date,
                        time: log_time,
                        method,
                        status: LedgerStatus::Skipped,
                        title: None,
                        source: canonical.clone(),
                            folder: None,
                    },
                )?;
                return Ok(IngestResult {
                    status: IngestStatus::Duplicate {
                        original_date: "inflight".to_string(),
                    },
                    method: Some(method),
                    canonical_url: Some(canonical),
                    ..Default::default()
                });
            }
            inflight.insert(canonical.clone());
        }

        // Then check ledger (durable dedup)
        if let Some(original_date) = ledger::check_duplicate(&ledger_file, &canonical)? {
            INFLIGHT.lock().await.remove(&canonical);
            log::info!("Duplicate URL: {canonical} (first ingested {original_date})");
            ledger::append_entry(
                &ledger_file,
                &LedgerEntry {
                    date: log_date,
                    time: log_time,
                    method,
                    status: LedgerStatus::Skipped,
                    title: None,
                    source: canonical.clone(),
                    folder: None,
                },
            )?;
            return Ok(IngestResult {
                status: IngestStatus::Duplicate { original_date },
                method: Some(method),
                canonical_url: Some(canonical),
                ..Default::default()
            });
        }
    }

    let url_match = router::classify_url(&canonical, &config.links)?;
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

    let mut all_tags: Vec<String> = tags.iter().map(|t| hygiene::sanitize_tag(t)).collect();

    // Generate tags via Fabric (graceful failure)
    if use_fabric && let Ok(fabric_tags) = fabric::generate_tags(&summary, &config.fabric).await {
        all_tags.extend(fabric_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
    }
    all_tags.sort();
    all_tags.dedup();

    // Resolve destination folder (3-tier routing)
    // If title is still "Unknown" after all extraction attempts, force fallback to Inbox
    let folder = if title == "Unknown" || title.is_empty() {
        log::warn!("Title is '{title}', forcing fallback to Inbox");
        config.routing.fallback_folder.clone()
    } else if !url_match.folder.is_empty() {
        // Tier 1: URL-type routing from config
        log::debug!("Tier 1 routing: URL config -> {}", url_match.folder);
        url_match.folder.clone()
    } else if use_fabric {
        // Tier 2: LLM topic classification
        match fabric::classify_topic(&title, &summary, &config.fabric).await {
            Ok(result) if result.confidence >= config.routing.confidence_threshold => {
                log::info!(
                    "Tier 2 routing: LLM classified -> {} (confidence: {:.2})",
                    result.folder,
                    result.confidence
                );
                all_tags.extend(result.suggested_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
                all_tags.sort();
                all_tags.dedup();
                result.folder
            }
            Ok(result) => {
                log::info!(
                    "Tier 2 routing: low confidence {:.2} for '{}', falling back",
                    result.confidence,
                    result.folder
                );
                config.routing.fallback_folder.clone()
            }
            Err(e) => {
                log::warn!("Tier 2 routing failed: {e:#}, using fallback");
                config.routing.fallback_folder.clone()
            }
        }
    } else {
        // Tier 3: Fallback
        log::debug!("Tier 3 routing: fallback -> {}", config.routing.fallback_folder);
        config.routing.fallback_folder.clone()
    };

    // Generate embed code for YouTube
    let embed_code = if url_match.is_youtube_type() {
        youtube::extract_video_id(&url_match.url)
            .map(|vid| youtube::generate_embed_code(&vid, url_match.width, url_match.height))
    } else {
        None
    };

    let note = NoteContent {
        title: title.clone(),
        source_url: url_match.url.clone(),
        tags: all_tags.clone(),
        summary,
        content_type,
        embed_code,
        method: Some(method),
    };

    let rendered = markdown::render_note(&note, &config.frontmatter);
    let filename = format!("{}.md", hygiene::sanitize_filename(&title));

    // Resolve write path
    let dest_path = resolve_destination(
        &config.vault.root_path,
        &config.vault.inbox_path,
        &folder,
        &config.routing,
    );
    std::fs::create_dir_all(&dest_path).context("Failed to create destination directory")?;

    let note_path = dest_path.join(&filename);
    std::fs::write(&note_path, &rendered).context("Failed to write note to vault")?;

    log::info!("Wrote note: {} (folder: {})", note_path.display(), folder);

    // Log success to Borg Ledger
    ledger::append_entry(
        &ledger_file,
        &LedgerEntry {
            date: log_date,
            time: log_time,
            method,
            status: LedgerStatus::Completed,
            title: Some(title.clone()),
            source: canonical.clone(),
            folder: Some(folder.clone()),
        },
    )?;

    // Release inflight guard now that ledger has the ✅ entry
    INFLIGHT.lock().await.remove(&canonical);

    Ok(IngestResult {
        status: IngestStatus::Completed,
        note_path: Some(note_path.to_string_lossy().to_string()),
        title: Some(title),
        tags: all_tags,
        elapsed_secs: None,
        folder: Some(folder),
        method: Some(method),
        canonical_url: Some(canonical),
    })
}

async fn process_youtube_fabric(url: &str, config: &Config) -> Result<(String, String, ContentType)> {
    let yt = fabric::fetch_youtube(url, &config.fabric).await?;

    // If fabric returned "Unknown" title, fall back to yt-dlp metadata
    let (title, channel, duration_secs) = if yt.title == "Unknown" || yt.title.is_empty() {
        log::warn!("Fabric returned no title, falling back to yt-dlp metadata");
        match youtube::fetch_metadata(url) {
            Ok(meta) => (meta.title, meta.uploader, meta.duration_secs),
            Err(e) => {
                log::warn!("yt-dlp metadata also failed: {e:#}");
                (yt.title, yt.channel, yt.duration_secs)
            }
        }
    } else {
        (yt.title, yt.channel, yt.duration_secs)
    };

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
        uploader: channel,
        duration_secs,
    };

    Ok((title, summary, content_type))
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

            let groq_key = crate::config::resolve_secret(&config.groq.api_key).ok();
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

    let title = extract_article_title(&article_md, url);

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

    let title = extract_article_title(&article_md, url);

    Ok((title, article_md, ContentType::Article))
}

fn resolve_destination(
    root_path: &str,
    inbox_path: &str,
    folder: &str,
    routing: &crate::config::RoutingConfig,
) -> PathBuf {
    if folder.is_empty() || folder == routing.fallback_folder {
        // Use inbox_path for fallback/Inbox
        return expand_tilde(inbox_path);
    }

    let root = expand_tilde(root_path);
    let mut dest = root.join(folder);

    // Add date subfolder for research content
    if routing.research_date_subfolder && folder.contains("research") {
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        dest = dest.join(date);
    }

    dest
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

    #[test]
    fn test_resolve_destination_fallback() {
        let routing = crate::config::RoutingConfig {
            fallback_folder: "Inbox".to_string(),
            ..Default::default()
        };
        let dest = resolve_destination("/vault", "/vault/Inbox", "Inbox", &routing);
        assert_eq!(dest, PathBuf::from("/vault/Inbox"));
    }

    #[test]
    fn test_resolve_destination_empty_folder() {
        let routing = crate::config::RoutingConfig::default();
        let dest = resolve_destination("/vault", "/vault/Inbox", "", &routing);
        assert_eq!(dest, PathBuf::from("/vault/Inbox"));
    }

    #[test]
    fn test_resolve_destination_specific_folder() {
        let routing = crate::config::RoutingConfig {
            research_date_subfolder: false,
            ..Default::default()
        };
        let dest = resolve_destination("/vault", "/vault/Inbox", "Tech/AI-LLM", &routing);
        assert_eq!(dest, PathBuf::from("/vault/Tech/AI-LLM"));
    }

    #[test]
    fn test_extract_title_from_fabric_metadata() {
        let md = "Title: Rust Programming Language\n\nURL Source: https://rust-lang.org\n\nMarkdown Content:\n# Rust\n";
        assert_eq!(
            extract_article_title(md, "https://rust-lang.org"),
            "Rust Programming Language"
        );
    }

    #[test]
    fn test_extract_title_from_pdf_filename() {
        let md = "Title: The-Complete-Guide-to-Building-Skill-for-Claude.pdf\n\nURL Source: https://example.com/doc.pdf\n\nMarkdown Content:\nThe Complete Guide\n\n# to Building Skills\n";
        assert_eq!(
            extract_article_title(md, "https://example.com/doc.pdf"),
            "The Complete Guide to Building Skill for Claude"
        );
    }

    #[test]
    fn test_extract_title_falls_back_to_heading() {
        let md = "Some random content\n# My Article Title\nBody text\n";
        assert_eq!(
            extract_article_title(md, "https://example.com/page"),
            "My Article Title"
        );
    }

    #[test]
    fn test_extract_title_falls_back_to_url_segment() {
        let md = "No title metadata here\nJust plain text\n";
        assert_eq!(
            extract_article_title(md, "https://example.com/my-great-article"),
            "my great article"
        );
    }

    #[test]
    fn test_resolve_destination_research_date_subfolder() {
        let routing = crate::config::RoutingConfig {
            research_date_subfolder: true,
            ..Default::default()
        };
        let dest = resolve_destination("/vault", "/vault/Inbox", "Football/research", &routing);
        // Should have a date subfolder
        let dest_str = dest.to_string_lossy();
        assert!(dest_str.starts_with("/vault/Football/research/"));
        // Date format: YYYY-MM-DD
        let date_part = dest_str.strip_prefix("/vault/Football/research/").expect("prefix");
        assert_eq!(date_part.len(), 10); // YYYY-MM-DD
    }
}
