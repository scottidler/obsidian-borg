use crate::assets;
use crate::config::Config;
use crate::extraction;
use crate::fabric;
use crate::hygiene;
use crate::jina;
use crate::ledger::{self, LedgerEntry, LedgerStatus};
use crate::markdown::{self, ContentType, NoteContent};
use crate::ocr;
use crate::router;
use crate::transcription::TranscriptionClient;
use crate::types::{AudioFormat, ContentKind, IngestMethod, IngestResult, IngestStatus};
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

/// Top-level pipeline entry point. Dispatches to type-specific handlers based on content kind.
pub async fn process_content(
    content: ContentKind,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> IngestResult {
    match content {
        ContentKind::Url(url) => process_url(&url, tags, method, force, config).await,
        ContentKind::Image { data, filename } => process_image(&data, &filename, tags, method, force, config).await,
        ContentKind::Pdf { data, filename } => {
            process_document_file(&data, &filename, tags, method, force, config, DocumentKind::Pdf).await
        }
        ContentKind::Audio { .. } => IngestResult {
            status: IngestStatus::Failed {
                reason: "Audio ingestion not yet implemented".to_string(),
            },
            method: Some(method),
            ..Default::default()
        },
        ContentKind::Text(text) => process_text(&text, tags, method, force, config).await,
        ContentKind::Document { data, filename } => {
            process_document_file(&data, &filename, tags, method, force, config, DocumentKind::Document).await
        }
    }
}

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
        source_url: Some(url_match.url.clone()),
        asset_path: None,
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

async fn process_image(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    _force: bool,
    config: &Config,
) -> IngestResult {
    let start = Instant::now();
    match process_image_inner(data, filename, tags, method, config).await {
        Ok(mut result) => {
            let elapsed = start.elapsed();
            log::info!("Image pipeline completed in {elapsed:.2?}");
            result.elapsed_secs = Some(elapsed.as_secs_f64());
            result
        }
        Err(e) => {
            let elapsed = start.elapsed();
            log::error!("Image pipeline failed in {elapsed:.2?}: {e:?}");
            IngestResult {
                status: IngestStatus::Failed {
                    reason: format!("{:#}", e),
                },
                method: Some(method),
                elapsed_secs: Some(elapsed.as_secs_f64()),
                ..Default::default()
            }
        }
    }
}

async fn process_image_inner(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    config: &Config,
) -> Result<IngestResult> {
    let tz: chrono_tz::Tz = config
        .frontmatter
        .timezone
        .parse()
        .unwrap_or(chrono_tz::America::Los_Angeles);
    let now = chrono::Utc::now().with_timezone(&tz);
    let log_date = now.format("%Y-%m-%d").to_string();
    let log_time = now.format("%H:%M").to_string();

    // Store asset in vault
    let date_bucket = chrono::Utc::now().format("%Y-%m").to_string();
    let subdirectory = format!("images/{date_bucket}");

    let vault_root = expand_tilde(&config.vault.root_path);
    let (_abs_path, rel_path) =
        assets::store_asset(&vault_root, data, filename, &subdirectory).context("Failed to store image asset")?;

    log::info!("Stored image asset: {rel_path}");

    // Write to temp file for OCR
    let temp_dir = std::env::temp_dir().join("obsidian-borg");
    std::fs::create_dir_all(&temp_dir).context("Failed to create temp directory")?;
    let temp_path = temp_dir.join(filename);
    std::fs::write(&temp_path, data).context("Failed to write temp image file")?;

    // OCR text extraction (best-effort)
    let ocr_text = ocr::ocr_extract(&temp_path).unwrap_or_else(|e| {
        log::warn!("OCR extraction failed: {e:#}");
        String::new()
    });

    if !ocr_text.is_empty() {
        log::debug!("OCR extracted {} chars", ocr_text.len());
    }

    // Generate title
    let use_fabric = fabric::is_available(&config.fabric);
    let title = if !ocr_text.is_empty() && ocr_text.len() > 5 {
        // Use first meaningful line from OCR as title candidate
        let first_line = ocr_text.lines().find(|l| l.trim().len() > 3).unwrap_or("").trim();
        if !first_line.is_empty() && first_line.len() <= 80 {
            first_line.to_string()
        } else {
            title_from_filename(filename)
        }
    } else {
        title_from_filename(filename)
    };

    let mut all_tags: Vec<String> = tags.iter().map(|t| hygiene::sanitize_tag(t)).collect();
    all_tags.push("image".to_string());

    // Generate tags via Fabric from OCR text or filename
    let tag_source = if !ocr_text.is_empty() {
        ocr_text.clone()
    } else {
        format!("Image file: {filename}")
    };

    if use_fabric && let Ok(fabric_tags) = fabric::generate_tags(&tag_source, &config.fabric).await {
        all_tags.extend(fabric_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
    }
    all_tags.sort();
    all_tags.dedup();

    // Classify topic for routing
    let summary_text = if !ocr_text.is_empty() {
        ocr_text.clone()
    } else {
        format!("Image: {}", title)
    };

    let folder = if use_fabric {
        match fabric::classify_topic(&title, &summary_text, &config.fabric).await {
            Ok(result) if result.confidence >= config.routing.confidence_threshold => {
                log::info!(
                    "Image routing: LLM classified -> {} (confidence: {:.2})",
                    result.folder,
                    result.confidence
                );
                all_tags.extend(result.suggested_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
                all_tags.sort();
                all_tags.dedup();
                result.folder
            }
            _ => config.routing.fallback_folder.clone(),
        }
    } else {
        config.routing.fallback_folder.clone()
    };

    // Build summary
    let summary = if !ocr_text.is_empty() {
        format!("## OCR Text\n\n{ocr_text}")
    } else {
        String::new()
    };

    let note = NoteContent {
        title: title.clone(),
        source_url: None,
        asset_path: Some(rel_path.clone()),
        tags: all_tags.clone(),
        summary,
        content_type: ContentType::Image { asset_path: rel_path },
        embed_code: None,
        method: Some(method),
    };

    let rendered = markdown::render_note(&note, &config.frontmatter);
    let note_filename = format!("{}.md", hygiene::sanitize_filename(&title));

    let dest_path = resolve_destination(
        &config.vault.root_path,
        &config.vault.inbox_path,
        &folder,
        &config.routing,
    );
    std::fs::create_dir_all(&dest_path).context("Failed to create destination directory")?;

    let note_path = dest_path.join(&note_filename);
    std::fs::write(&note_path, &rendered).context("Failed to write image note to vault")?;

    log::info!("Wrote image note: {} (folder: {})", note_path.display(), folder);

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    // Log to ledger
    let ledger_file = ledger::ledger_path(config);
    let source_display = format!("[image: {filename}]");
    ledger::append_entry(
        &ledger_file,
        &LedgerEntry {
            date: log_date,
            time: log_time,
            method,
            status: LedgerStatus::Completed,
            title: Some(title.clone()),
            source: source_display,
            folder: Some(folder.clone()),
        },
    )?;

    Ok(IngestResult {
        status: IngestStatus::Completed,
        note_path: Some(note_path.to_string_lossy().to_string()),
        title: Some(title),
        tags: all_tags,
        elapsed_secs: None,
        folder: Some(folder),
        method: Some(method),
        canonical_url: None,
    })
}

fn title_from_filename(filename: &str) -> String {
    let stem = filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(filename);
    let cleaned = stem.replace(['-', '_'], " ");
    if cleaned.trim().is_empty() {
        "Untitled Image".to_string()
    } else {
        cleaned.trim().to_string()
    }
}

/// Whether a file-based document is a PDF or a generic document (docx, pptx, etc.).
#[derive(Debug, Clone, Copy)]
enum DocumentKind {
    Pdf,
    Document,
}

impl DocumentKind {
    fn subdirectory(self) -> &'static str {
        match self {
            DocumentKind::Pdf => "pdfs",
            DocumentKind::Document => "docs",
        }
    }

    fn label(self) -> &'static str {
        match self {
            DocumentKind::Pdf => "pdf",
            DocumentKind::Document => "document",
        }
    }

    fn default_tag(self) -> &'static str {
        match self {
            DocumentKind::Pdf => "pdf",
            DocumentKind::Document => "document",
        }
    }

    fn content_type(self, asset_path: String) -> ContentType {
        match self {
            DocumentKind::Pdf => ContentType::Pdf { asset_path },
            DocumentKind::Document => ContentType::Document { asset_path },
        }
    }
}

async fn process_document_file(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    _force: bool,
    config: &Config,
    kind: DocumentKind,
) -> IngestResult {
    let start = Instant::now();
    match process_document_file_inner(data, filename, tags, method, config, kind).await {
        Ok(mut result) => {
            let elapsed = start.elapsed();
            log::info!("{} pipeline completed in {elapsed:.2?}", kind.label().to_uppercase());
            result.elapsed_secs = Some(elapsed.as_secs_f64());
            result
        }
        Err(e) => {
            let elapsed = start.elapsed();
            log::error!(
                "{} pipeline failed in {elapsed:.2?}: {e:?}",
                kind.label().to_uppercase()
            );
            IngestResult {
                status: IngestStatus::Failed {
                    reason: format!("{:#}", e),
                },
                method: Some(method),
                elapsed_secs: Some(elapsed.as_secs_f64()),
                ..Default::default()
            }
        }
    }
}

async fn process_document_file_inner(
    data: &[u8],
    filename: &str,
    tags: Vec<String>,
    method: IngestMethod,
    config: &Config,
    kind: DocumentKind,
) -> Result<IngestResult> {
    let tz: chrono_tz::Tz = config
        .frontmatter
        .timezone
        .parse()
        .unwrap_or(chrono_tz::America::Los_Angeles);
    let now = chrono::Utc::now().with_timezone(&tz);
    let log_date = now.format("%Y-%m-%d").to_string();
    let log_time = now.format("%H:%M").to_string();

    // Store asset in vault
    let vault_root = expand_tilde(&config.vault.root_path);
    let (_abs_path, rel_path) = assets::store_asset(&vault_root, data, filename, kind.subdirectory())
        .context(format!("Failed to store {} asset", kind.label()))?;

    log::info!("Stored {} asset: {rel_path}", kind.label());

    // Write to temp file for text extraction
    let temp_dir = std::env::temp_dir().join("obsidian-borg");
    std::fs::create_dir_all(&temp_dir).context("Failed to create temp directory")?;
    let temp_path = temp_dir.join(filename);
    std::fs::write(&temp_path, data).context("Failed to write temp file")?;

    // Extract text via markitdown-cli
    let extracted_text = extraction::extract_markdown(&temp_path).unwrap_or_else(|e| {
        log::warn!("Text extraction failed for {filename}: {e:#}");
        String::new()
    });

    if !extracted_text.is_empty() {
        log::debug!("Extracted {} chars from {}", extracted_text.len(), filename);
    }

    // Generate title
    let use_fabric = fabric::is_available(&config.fabric);
    let title = if !extracted_text.is_empty() {
        // Use extract_article_title logic - look for a good title from the extracted text
        let title_candidate = extracted_text
            .lines()
            .find(|l| {
                let trimmed = l.trim();
                !trimmed.is_empty() && trimmed.len() > 3 && !trimmed.starts_with("Title:")
            })
            .map(|l| l.trim().to_string());

        // Check for a Title: line first
        let md_title = extracted_text
            .lines()
            .find(|line| line.starts_with("Title:"))
            .map(|line| line.trim_start_matches("Title:").trim().to_string())
            .filter(|t| !t.is_empty());

        // Check for a # heading
        let heading_title = extracted_text
            .lines()
            .find(|line| line.starts_with("# "))
            .map(|line| line.trim_start_matches("# ").trim().to_string())
            .filter(|t| !t.is_empty());

        md_title
            .or(heading_title)
            .or(title_candidate)
            .unwrap_or_else(|| title_from_filename(filename))
    } else {
        title_from_filename(filename)
    };

    let mut all_tags: Vec<String> = tags.iter().map(|t| hygiene::sanitize_tag(t)).collect();
    all_tags.push(kind.default_tag().to_string());

    // Summarize via fabric
    let summary = if use_fabric && !extracted_text.is_empty() {
        match fabric::summarize(&extracted_text, false, &config.fabric).await {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Fabric summarize failed: {e:#}");
                if extracted_text.len() > 500 {
                    extracted_text[..500].to_string()
                } else {
                    extracted_text.clone()
                }
            }
        }
    } else if !extracted_text.is_empty() {
        // No fabric - use a truncated extract
        if extracted_text.len() > 1000 {
            format!("{}...", &extracted_text[..1000])
        } else {
            extracted_text.clone()
        }
    } else {
        String::new()
    };

    // Generate tags via Fabric
    let tag_source = if !extracted_text.is_empty() {
        extracted_text.clone()
    } else {
        format!("{} file: {filename}", kind.label())
    };

    if use_fabric && let Ok(fabric_tags) = fabric::generate_tags(&tag_source, &config.fabric).await {
        all_tags.extend(fabric_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
    }
    all_tags.sort();
    all_tags.dedup();

    // Classify topic for routing
    let summary_for_classify = if !extracted_text.is_empty() {
        extracted_text.clone()
    } else {
        format!("{}: {}", kind.label(), title)
    };

    let folder = if use_fabric {
        match fabric::classify_topic(&title, &summary_for_classify, &config.fabric).await {
            Ok(result) if result.confidence >= config.routing.confidence_threshold => {
                log::info!(
                    "{} routing: LLM classified -> {} (confidence: {:.2})",
                    kind.label(),
                    result.folder,
                    result.confidence
                );
                all_tags.extend(result.suggested_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
                all_tags.sort();
                all_tags.dedup();
                result.folder
            }
            _ => config.routing.fallback_folder.clone(),
        }
    } else {
        config.routing.fallback_folder.clone()
    };

    let note = NoteContent {
        title: title.clone(),
        source_url: None,
        asset_path: Some(rel_path.clone()),
        tags: all_tags.clone(),
        summary,
        content_type: kind.content_type(rel_path),
        embed_code: None,
        method: Some(method),
    };

    let rendered = markdown::render_note(&note, &config.frontmatter);
    let note_filename = format!("{}.md", hygiene::sanitize_filename(&title));

    let dest_path = resolve_destination(
        &config.vault.root_path,
        &config.vault.inbox_path,
        &folder,
        &config.routing,
    );
    std::fs::create_dir_all(&dest_path).context("Failed to create destination directory")?;

    let note_path = dest_path.join(&note_filename);
    std::fs::write(&note_path, &rendered).context(format!("Failed to write {} note to vault", kind.label()))?;

    log::info!(
        "Wrote {} note: {} (folder: {})",
        kind.label(),
        note_path.display(),
        folder
    );

    // Clean up temp file
    let _ = std::fs::remove_file(&temp_path);

    // Log to ledger
    let ledger_file = ledger::ledger_path(config);
    let source_display = format!("[{}: {filename}]", kind.label());
    ledger::append_entry(
        &ledger_file,
        &LedgerEntry {
            date: log_date,
            time: log_time,
            method,
            status: LedgerStatus::Completed,
            title: Some(title.clone()),
            source: source_display,
            folder: Some(folder.clone()),
        },
    )?;

    Ok(IngestResult {
        status: IngestStatus::Completed,
        note_path: Some(note_path.to_string_lossy().to_string()),
        title: Some(title),
        tags: all_tags,
        elapsed_secs: None,
        folder: Some(folder),
        method: Some(method),
        canonical_url: None,
    })
}

/// Detect structured text patterns before LLM classification.
#[derive(Debug, PartialEq)]
enum TextPattern {
    Define { word: String },
    Clarify { word_a: String, word_b: String },
    ContainsUrl(String),
    General,
}

fn detect_text_pattern(text: &str) -> TextPattern {
    let trimmed = text.trim();

    // Check for define: pattern
    if let Some(word) = trimmed
        .strip_prefix("define:")
        .or_else(|| trimmed.strip_prefix("Define:"))
        .map(|w| w.trim().to_string())
        && !word.is_empty()
    {
        return TextPattern::Define { word };
    }

    // Check for clarify: <word> vs <word> pattern
    if let Some(rest) = trimmed
        .strip_prefix("clarify:")
        .or_else(|| trimmed.strip_prefix("Clarify:"))
        .map(|w| w.trim())
        && let Some((a, b)) = rest.split_once(" vs ")
    {
        let word_a = a.trim().to_string();
        let word_b = b.trim().to_string();
        if !word_a.is_empty() && !word_b.is_empty() {
            return TextPattern::Clarify { word_a, word_b };
        }
    }

    // Check if text contains a URL (redirect to URL pipeline)
    if let Some(url) = router::extract_url_from_text(trimmed) {
        // Only redirect if the text IS essentially just a URL
        let without_url = trimmed.replace(&url, "").trim().to_string();
        if without_url.is_empty() || without_url.len() < 10 {
            return TextPattern::ContainsUrl(url);
        }
    }

    TextPattern::General
}

async fn process_text(
    text: &str,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> IngestResult {
    let start = Instant::now();
    match process_text_inner(text, tags, method, force, config).await {
        Ok(mut result) => {
            let elapsed = start.elapsed();
            log::info!("Text pipeline completed in {elapsed:.2?}");
            result.elapsed_secs = Some(elapsed.as_secs_f64());
            result
        }
        Err(e) => {
            let elapsed = start.elapsed();
            log::error!("Text pipeline failed in {elapsed:.2?}: {e:?}");
            IngestResult {
                status: IngestStatus::Failed {
                    reason: format!("{:#}", e),
                },
                method: Some(method),
                elapsed_secs: Some(elapsed.as_secs_f64()),
                ..Default::default()
            }
        }
    }
}

async fn process_text_inner(
    text: &str,
    tags: Vec<String>,
    method: IngestMethod,
    force: bool,
    config: &Config,
) -> Result<IngestResult> {
    let pattern = detect_text_pattern(text);
    log::debug!("Text pattern detected: {pattern:?}");

    match pattern {
        TextPattern::ContainsUrl(url) => {
            // Redirect to URL pipeline
            return Ok(process_url(&url, tags, method, force, config).await);
        }
        TextPattern::Define { .. } | TextPattern::Clarify { .. } => {
            return process_vocab(text, &pattern, tags, method, force, config).await;
        }
        TextPattern::General => {}
    }

    // General text: classify via LLM, then create a note
    let tz: chrono_tz::Tz = config
        .frontmatter
        .timezone
        .parse()
        .unwrap_or(chrono_tz::America::Los_Angeles);
    let now = chrono::Utc::now().with_timezone(&tz);
    let log_date = now.format("%Y-%m-%d").to_string();
    let log_time = now.format("%H:%M").to_string();

    let use_fabric = fabric::is_available(&config.fabric);

    // Generate title from text (first line or LLM-generated)
    let title = generate_text_title(text, use_fabric, config).await;

    let mut all_tags: Vec<String> = tags.iter().map(|t| hygiene::sanitize_tag(t)).collect();

    // Generate tags via Fabric
    if use_fabric && let Ok(fabric_tags) = fabric::generate_tags(text, &config.fabric).await {
        all_tags.extend(fabric_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
    }
    all_tags.sort();
    all_tags.dedup();

    // Route via LLM classification
    let folder = if use_fabric {
        match fabric::classify_topic(&title, text, &config.fabric).await {
            Ok(result) if result.confidence >= config.routing.confidence_threshold => {
                log::info!(
                    "Text routing: LLM classified -> {} (confidence: {:.2})",
                    result.folder,
                    result.confidence
                );
                all_tags.extend(result.suggested_tags.into_iter().map(|t| hygiene::sanitize_tag(&t)));
                all_tags.sort();
                all_tags.dedup();
                result.folder
            }
            _ => config.routing.fallback_folder.clone(),
        }
    } else {
        config.routing.fallback_folder.clone()
    };

    let note = NoteContent {
        title: title.clone(),
        source_url: None,
        asset_path: None,
        tags: all_tags.clone(),
        summary: text.to_string(),
        content_type: ContentType::Note,
        embed_code: None,
        method: Some(method),
    };

    let rendered = markdown::render_note(&note, &config.frontmatter);
    let filename = format!("{}.md", hygiene::sanitize_filename(&title));

    let dest_path = resolve_destination(
        &config.vault.root_path,
        &config.vault.inbox_path,
        &folder,
        &config.routing,
    );
    std::fs::create_dir_all(&dest_path).context("Failed to create destination directory")?;

    let note_path = dest_path.join(&filename);
    std::fs::write(&note_path, &rendered).context("Failed to write note to vault")?;

    log::info!("Wrote text note: {} (folder: {})", note_path.display(), folder);

    // Log to ledger
    let ledger_file = ledger::ledger_path(config);
    let source_display = format!(
        "[text: {}]",
        if text.len() > 50 { format!("{}...", &text[..50]) } else { text.to_string() }
    );
    ledger::append_entry(
        &ledger_file,
        &LedgerEntry {
            date: log_date,
            time: log_time,
            method,
            status: LedgerStatus::Completed,
            title: Some(title.clone()),
            source: source_display,
            folder: Some(folder.clone()),
        },
    )?;

    Ok(IngestResult {
        status: IngestStatus::Completed,
        note_path: Some(note_path.to_string_lossy().to_string()),
        title: Some(title),
        tags: all_tags,
        elapsed_secs: None,
        folder: Some(folder),
        method: Some(method),
        canonical_url: None,
    })
}

async fn process_vocab(
    text: &str,
    pattern: &TextPattern,
    tags: Vec<String>,
    method: IngestMethod,
    _force: bool,
    config: &Config,
) -> Result<IngestResult> {
    let tz: chrono_tz::Tz = config
        .frontmatter
        .timezone
        .parse()
        .unwrap_or(chrono_tz::America::Los_Angeles);
    let now = chrono::Utc::now().with_timezone(&tz);
    let log_date = now.format("%Y-%m-%d").to_string();
    let log_time = now.format("%H:%M").to_string();

    let use_fabric = fabric::is_available(&config.fabric);

    let (title, content_type, body, folder) = match pattern {
        TextPattern::Define { word } => {
            // Generate definition via LLM
            let body = if use_fabric {
                let prompt = format!(
                    "Define the word \"{word}\". Determine what language it is. \
                     Provide: 1) The definition 2) Example sentences. \
                     Format as markdown with ## Examples section."
                );
                fabric::run_pattern("summarize", &prompt, &config.fabric)
                    .await
                    .unwrap_or_else(|_| format!("definition:: [define: {word}]"))
            } else {
                format!("definition:: [define: {word}]")
            };

            // Detect language (simple heuristic: ask LLM or check common patterns)
            let language = detect_language(word, use_fabric, config).await;
            let folder = resolve_vocab_folder(&language, &config.text_capture);

            (
                word.clone(),
                ContentType::VocabDefine {
                    word: word.clone(),
                    language: language.clone(),
                },
                body,
                folder,
            )
        }
        TextPattern::Clarify { word_a, word_b } => {
            let title = format!("{word_a} vs {word_b}");
            let body = if use_fabric {
                let prompt = format!(
                    "Compare and clarify the difference between \"{word_a}\" and \"{word_b}\". \
                     Determine what language they are. \
                     Provide: definitions, usage contexts, examples, and common confusions. \
                     Format as markdown."
                );
                fabric::run_pattern("summarize", &prompt, &config.fabric)
                    .await
                    .unwrap_or_else(|_| format!("[clarify: {word_a} vs {word_b}]"))
            } else {
                format!("[clarify: {word_a} vs {word_b}]")
            };

            let language = detect_language(word_a, use_fabric, config).await;
            let folder = resolve_vocab_folder(&language, &config.text_capture);

            (
                title,
                ContentType::VocabClarify {
                    word_a: word_a.clone(),
                    word_b: word_b.clone(),
                    language: language.clone(),
                },
                body,
                folder,
            )
        }
        _ => unreachable!("process_vocab called with non-vocab pattern"),
    };

    let mut all_tags: Vec<String> = tags.iter().map(|t| hygiene::sanitize_tag(t)).collect();
    let vocab_tag = match &content_type {
        ContentType::VocabDefine { language, .. } | ContentType::VocabClarify { language, .. } => {
            format!("{language}-vocab")
        }
        _ => "vocab".to_string(),
    };
    all_tags.push(hygiene::sanitize_tag(&vocab_tag));
    all_tags.sort();
    all_tags.dedup();

    let note = NoteContent {
        title: title.clone(),
        source_url: None,
        asset_path: None,
        tags: all_tags.clone(),
        summary: body,
        content_type,
        embed_code: None,
        method: Some(method),
    };

    let rendered = markdown::render_note(&note, &config.frontmatter);
    let filename = format!("{}.md", hygiene::sanitize_filename(&title));

    let dest_path = resolve_destination(
        &config.vault.root_path,
        &config.vault.inbox_path,
        &folder,
        &config.routing,
    );
    std::fs::create_dir_all(&dest_path).context("Failed to create destination directory")?;

    let note_path = dest_path.join(&filename);
    std::fs::write(&note_path, &rendered).context("Failed to write note to vault")?;

    log::info!("Wrote vocab note: {} (folder: {})", note_path.display(), folder);

    // Log to ledger
    let ledger_file = ledger::ledger_path(config);
    ledger::append_entry(
        &ledger_file,
        &LedgerEntry {
            date: log_date,
            time: log_time,
            method,
            status: LedgerStatus::Completed,
            title: Some(title.clone()),
            source: format!("[{}]", text.trim()),
            folder: Some(folder.clone()),
        },
    )?;

    Ok(IngestResult {
        status: IngestStatus::Completed,
        note_path: Some(note_path.to_string_lossy().to_string()),
        title: Some(title),
        tags: all_tags,
        elapsed_secs: None,
        folder: Some(folder),
        method: Some(method),
        canonical_url: None,
    })
}

/// Generate a title from text input.
async fn generate_text_title(text: &str, use_fabric: bool, config: &Config) -> String {
    // Use first line as title if it's short enough
    let first_line = text.lines().next().unwrap_or(text).trim();
    if !first_line.is_empty() && first_line.len() <= 80 {
        return first_line.to_string();
    }

    // Try LLM to generate a title
    if use_fabric
        && let Ok(title) = fabric::run_pattern(
            "summarize",
            &format!("Generate a very short (3-8 word) title for this text:\n\n{text}"),
            &config.fabric,
        )
        .await
    {
        let title = title.lines().next().unwrap_or(&title).trim().to_string();
        if !title.is_empty() && title.len() <= 100 {
            return title;
        }
    }

    // Fallback: truncate first line
    if first_line.len() > 80 {
        format!("{}...", &first_line[..77])
    } else {
        "Quick Note".to_string()
    }
}

/// Detect language of a word (simple heuristic, can be enhanced with LLM).
async fn detect_language(word: &str, use_fabric: bool, config: &Config) -> String {
    if use_fabric
        && let Ok(result) = fabric::run_pattern(
            "summarize",
            &format!(
                "What language is the word \"{word}\"? Reply with just the language name \
                 in lowercase (e.g., \"english\", \"spanish\", \"french\"). Nothing else."
            ),
            &config.fabric,
        )
        .await
    {
        let lang = result.trim().to_lowercase();
        // Accept reasonable language names
        if !lang.is_empty() && lang.len() < 20 && !lang.contains(' ') {
            return lang;
        }
    }

    // Fallback: assume English
    "english".to_string()
}

fn resolve_vocab_folder(language: &str, text_capture: &crate::config::TextCaptureConfig) -> String {
    text_capture
        .vocab_folders
        .get(language)
        .or_else(|| text_capture.vocab_folders.get("default"))
        .cloned()
        .unwrap_or_else(|| "🧠 Knowledge/vocab".to_string())
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
    fn test_detect_text_pattern_define() {
        assert_eq!(
            detect_text_pattern("define: garrulous"),
            TextPattern::Define {
                word: "garrulous".to_string()
            }
        );
        assert_eq!(
            detect_text_pattern("Define: escurrir"),
            TextPattern::Define {
                word: "escurrir".to_string()
            }
        );
    }

    #[test]
    fn test_detect_text_pattern_clarify() {
        assert_eq!(
            detect_text_pattern("clarify: affect vs effect"),
            TextPattern::Clarify {
                word_a: "affect".to_string(),
                word_b: "effect".to_string()
            }
        );
        assert_eq!(
            detect_text_pattern("Clarify: escurrir vs estrujar"),
            TextPattern::Clarify {
                word_a: "escurrir".to_string(),
                word_b: "estrujar".to_string()
            }
        );
    }

    #[test]
    fn test_detect_text_pattern_url() {
        match detect_text_pattern("https://example.com") {
            TextPattern::ContainsUrl(url) => assert_eq!(url, "https://example.com"),
            other => panic!("expected ContainsUrl, got {other:?}"),
        }
    }

    #[test]
    fn test_detect_text_pattern_url_with_short_context() {
        // URL with very short surrounding text should still be treated as URL
        match detect_text_pattern("check https://example.com") {
            TextPattern::ContainsUrl(url) => assert_eq!(url, "https://example.com"),
            other => panic!("expected ContainsUrl, got {other:?}"),
        }
    }

    #[test]
    fn test_detect_text_pattern_general() {
        assert_eq!(
            detect_text_pattern("Met James at the Rust meetup"),
            TextPattern::General
        );
    }

    #[test]
    fn test_detect_text_pattern_empty_define() {
        // "define:" with no word should not match
        assert_eq!(detect_text_pattern("define: "), TextPattern::General);
    }

    #[test]
    fn test_resolve_vocab_folder_english() {
        let config = crate::config::TextCaptureConfig::default();
        assert_eq!(resolve_vocab_folder("english", &config), "🧠 Knowledge/english-vocab");
    }

    #[test]
    fn test_resolve_vocab_folder_spanish() {
        let config = crate::config::TextCaptureConfig::default();
        assert_eq!(resolve_vocab_folder("spanish", &config), "🇪🇸 Spanish/vocabulary");
    }

    #[test]
    fn test_resolve_vocab_folder_unknown() {
        let config = crate::config::TextCaptureConfig::default();
        assert_eq!(resolve_vocab_folder("french", &config), "🧠 Knowledge/vocab");
    }

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

    #[tokio::test]
    async fn test_process_content_unsupported_types() {
        use crate::types::ContentKind;

        let config = crate::config::Config::default();

        // Image is now implemented (Phase 3), tested separately
        // PDF is now implemented (Phase 4), tested separately
        // Document is now implemented (Phase 4), tested separately

        // Audio (still not implemented)
        let result = super::process_content(
            ContentKind::Audio {
                data: vec![1, 2, 3],
                filename: "test.mp3".to_string(),
            },
            vec![],
            IngestMethod::Cli,
            false,
            &config,
        )
        .await;
        assert!(matches!(result.status, IngestStatus::Failed { .. }));
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
