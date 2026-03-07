use std::sync::LazyLock;

use crate::config::LinkConfig;
use crate::types::{IngestResult, IngestStatus};
use crate::url_hygiene;

static URL_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"https?://\S+").expect("valid regex"));

const RESOLUTIONS: &[(&str, (usize, usize))] = &[
    ("nHD", (640, 360)),
    ("FWVGA", (854, 480)),
    ("SD", (1280, 720)),
    ("FHD", (1920, 1080)),
    ("4K", (3840, 2160)),
];

const SHORTS_RESOLUTIONS: &[(&str, (usize, usize))] =
    &[("480p", (480, 854)), ("720p", (720, 1280)), ("1080p", (1080, 1920))];

#[derive(Debug, PartialEq)]
pub struct UrlMatch {
    pub url: String,
    pub link_name: String,
    pub folder: String,
    pub width: usize,
    pub height: usize,
}

impl UrlMatch {
    pub fn is_youtube_type(&self) -> bool {
        matches!(self.link_name.as_str(), "youtube" | "shorts")
    }

    pub fn is_shorts(&self) -> bool {
        self.link_name == "shorts"
    }
}

pub fn classify_url(raw_url: &str, links: &[LinkConfig]) -> eyre::Result<UrlMatch> {
    let cleaned = url_hygiene::clean_url(raw_url)?;

    for link in links {
        let re = regex::Regex::new(&link.regex)?;
        if re.is_match(&cleaned) {
            let is_shorts = link.name == "shorts";
            let (width, height) = resolve_dimensions(&link.resolution, is_shorts);
            return Ok(UrlMatch {
                url: cleaned,
                link_name: link.name.clone(),
                folder: link.folder.clone(),
                width,
                height,
            });
        }
    }

    // Should not happen if config has a catch-all, but fallback
    Ok(UrlMatch {
        url: cleaned,
        link_name: "default".to_string(),
        folder: String::new(),
        width: 854,
        height: 480,
    })
}

fn resolve_dimensions(resolution: &str, is_shorts: bool) -> (usize, usize) {
    let table = if is_shorts { SHORTS_RESOLUTIONS } else { RESOLUTIONS };
    table
        .iter()
        .find(|(name, _)| *name == resolution)
        .map(|(_, dims)| *dims)
        .unwrap_or(if is_shorts { (480, 854) } else { (854, 480) })
}

pub fn extract_url_from_text(text: &str) -> Option<String> {
    URL_REGEX.find(text).map(|m| {
        m.as_str()
            .trim_end_matches(['.', ',', ')', ']', '>', ';', '!'])
            .to_string()
    })
}

pub fn format_reply(result: &IngestResult, url: &str) -> String {
    let elapsed = result.elapsed_secs.map(|s| format!(" ({:.1}s)", s)).unwrap_or_default();

    match &result.status {
        IngestStatus::Completed => {
            let title = result.title.as_deref().unwrap_or("Untitled");
            let tags = if result.tags.is_empty() {
                String::new()
            } else {
                format!(
                    "\nTags: {}",
                    result
                        .tags
                        .iter()
                        .map(|t| format!("#{t}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let folder_info = result
                .folder
                .as_ref()
                .map(|f| format!("\nFolder: {f}"))
                .unwrap_or_default();
            format!("Saved: {title}{elapsed}{tags}{folder_info}")
        }
        IngestStatus::Failed { reason } => {
            format!("Failed{elapsed}: {reason}\nURL: {url}")
        }
        IngestStatus::Queued => "Queued for processing.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_links() -> Vec<LinkConfig> {
        vec![
            LinkConfig {
                name: "shorts".to_string(),
                regex: r"https?://(?:www\.)?youtube\.com/shorts/([a-zA-Z0-9_-]+)".to_string(),
                resolution: "480p".to_string(),
                folder: "".to_string(),
            },
            LinkConfig {
                name: "youtube".to_string(),
                regex: r"https?://(?:www\.)?(youtube\.com/watch\?v=|youtu\.be/|music\.youtube\.com/watch\?v=)([a-zA-Z0-9_-]+)".to_string(),
                resolution: "FWVGA".to_string(),
                folder: "".to_string(),
            },
            LinkConfig {
                name: "default".to_string(),
                regex: r".*".to_string(),
                resolution: "FWVGA".to_string(),
                folder: "".to_string(),
            },
        ]
    }

    #[test]
    fn test_youtube_url() {
        let result = classify_url("https://www.youtube.com/watch?v=abc123", &test_links()).expect("valid");
        assert_eq!(result.link_name, "youtube");
        assert!(result.is_youtube_type());
        assert_eq!(result.width, 854);
        assert_eq!(result.height, 480);
    }

    #[test]
    fn test_youtube_short_url() {
        let result = classify_url("https://youtu.be/abc123", &test_links()).expect("valid");
        assert_eq!(result.link_name, "youtube");
        assert!(result.is_youtube_type());
    }

    #[test]
    fn test_youtube_music_url() {
        let result = classify_url("https://music.youtube.com/watch?v=abc123", &test_links()).expect("valid");
        assert_eq!(result.link_name, "youtube");
        assert!(result.is_youtube_type());
    }

    #[test]
    fn test_youtube_shorts() {
        let result = classify_url("https://youtube.com/shorts/abc123", &test_links()).expect("valid");
        assert_eq!(result.link_name, "shorts");
        assert!(result.is_shorts());
        assert_eq!(result.width, 480);
        assert_eq!(result.height, 854);
    }

    #[test]
    fn test_article_url() {
        let result = classify_url("https://blog.example.com/post", &test_links()).expect("valid");
        assert_eq!(result.link_name, "default");
        assert!(!result.is_youtube_type());
    }

    #[test]
    fn test_invalid_url() {
        let result = classify_url("not a url", &test_links());
        assert!(result.is_err());
    }

    #[test]
    fn test_url_cleaning_integrated() {
        let result = classify_url(
            "https://www.youtube.com/watch?v=abc&utm_source=twitter&si=track",
            &test_links(),
        )
        .expect("valid");
        assert_eq!(result.url, "https://www.youtube.com/watch?v=abc");
        assert_eq!(result.link_name, "youtube");
    }

    #[test]
    fn test_custom_folder() {
        let links = vec![LinkConfig {
            name: "youtube".to_string(),
            regex: r"https?://(?:www\.)?youtube\.com/watch".to_string(),
            resolution: "FHD".to_string(),
            folder: "Videos".to_string(),
        }];
        let result = classify_url("https://www.youtube.com/watch?v=abc", &links).expect("valid");
        assert_eq!(result.folder, "Videos");
        assert_eq!(result.width, 1920);
        assert_eq!(result.height, 1080);
    }

    #[test]
    fn test_resolve_dimensions_landscape() {
        assert_eq!(resolve_dimensions("nHD", false), (640, 360));
        assert_eq!(resolve_dimensions("FWVGA", false), (854, 480));
        assert_eq!(resolve_dimensions("FHD", false), (1920, 1080));
        assert_eq!(resolve_dimensions("4K", false), (3840, 2160));
    }

    #[test]
    fn test_resolve_dimensions_shorts() {
        assert_eq!(resolve_dimensions("480p", true), (480, 854));
        assert_eq!(resolve_dimensions("720p", true), (720, 1280));
        assert_eq!(resolve_dimensions("1080p", true), (1080, 1920));
    }

    #[test]
    fn test_resolve_dimensions_unknown() {
        assert_eq!(resolve_dimensions("unknown", false), (854, 480));
        assert_eq!(resolve_dimensions("unknown", true), (480, 854));
    }

    #[test]
    fn test_extract_bare_url() {
        let result = extract_url_from_text("https://example.com/page");
        assert_eq!(result, Some("https://example.com/page".to_string()));
    }

    #[test]
    fn test_extract_url_in_sentence() {
        let result = extract_url_from_text("check this out https://example.com/page please");
        assert_eq!(result, Some("https://example.com/page".to_string()));
    }

    #[test]
    fn test_extract_url_trailing_punctuation() {
        let result = extract_url_from_text("See https://example.com/page.");
        assert_eq!(result, Some("https://example.com/page".to_string()));
    }

    #[test]
    fn test_extract_url_trailing_paren() {
        let result = extract_url_from_text("(https://example.com/page)");
        assert_eq!(result, Some("https://example.com/page".to_string()));
    }

    #[test]
    fn test_extract_no_url() {
        let result = extract_url_from_text("no urls here");
        assert_eq!(result, None);
    }

    #[test]
    fn test_extract_multiple_urls_takes_first() {
        let result = extract_url_from_text("https://first.com and https://second.com");
        assert_eq!(result, Some("https://first.com".to_string()));
    }

    #[test]
    fn test_format_reply_completed() {
        let result = IngestResult {
            status: IngestStatus::Completed,
            note_path: Some("/vault/Inbox/Test.md".to_string()),
            title: Some("Test Article".to_string()),
            tags: vec!["ai".to_string(), "tech".to_string()],
            elapsed_secs: Some(4.56),
            folder: None,
        };
        let reply = format_reply(&result, "https://example.com");
        assert_eq!(reply, "Saved: Test Article (4.6s)\nTags: #ai, #tech");
    }

    #[test]
    fn test_format_reply_completed_with_folder() {
        let result = IngestResult {
            status: IngestStatus::Completed,
            note_path: Some("/vault/Tech/Test.md".to_string()),
            title: Some("Test".to_string()),
            tags: vec![],
            elapsed_secs: None,
            folder: Some("Tech/AI-LLM".to_string()),
        };
        let reply = format_reply(&result, "https://example.com");
        assert_eq!(reply, "Saved: Test\nFolder: Tech/AI-LLM");
    }

    #[test]
    fn test_format_reply_completed_no_tags() {
        let result = IngestResult {
            status: IngestStatus::Completed,
            note_path: None,
            title: Some("Test".to_string()),
            tags: vec![],
            elapsed_secs: None,
            folder: None,
        };
        let reply = format_reply(&result, "https://example.com");
        assert_eq!(reply, "Saved: Test");
    }

    #[test]
    fn test_format_reply_failed() {
        let result = IngestResult {
            status: IngestStatus::Failed {
                reason: "network error".to_string(),
            },
            elapsed_secs: Some(2.3),
            ..Default::default()
        };
        let reply = format_reply(&result, "https://example.com/broken");
        assert_eq!(reply, "Failed (2.3s): network error\nURL: https://example.com/broken");
    }

    #[test]
    fn test_format_reply_queued() {
        let result = IngestResult {
            status: IngestStatus::Queued,
            ..Default::default()
        };
        let reply = format_reply(&result, "https://example.com");
        assert_eq!(reply, "Queued for processing.");
    }
}
