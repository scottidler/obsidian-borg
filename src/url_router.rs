use std::sync::LazyLock;

use crate::types::{IngestResult, IngestStatus};
use url::Url;

static URL_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"https?://\S+").expect("valid regex"));

#[derive(Debug, PartialEq)]
pub enum UrlType {
    YouTube(String),
    Article(String),
}

pub fn classify_url(raw_url: &str) -> eyre::Result<UrlType> {
    let parsed = Url::parse(raw_url)?;
    let host = parsed.host_str().unwrap_or("");

    if is_youtube_host(host) {
        Ok(UrlType::YouTube(raw_url.to_string()))
    } else {
        Ok(UrlType::Article(raw_url.to_string()))
    }
}

fn is_youtube_host(host: &str) -> bool {
    matches!(
        host,
        "youtube.com" | "www.youtube.com" | "m.youtube.com" | "youtu.be" | "music.youtube.com"
    )
}

pub fn extract_url_from_text(text: &str) -> Option<String> {
    URL_REGEX.find(text).map(|m| {
        m.as_str()
            .trim_end_matches(['.', ',', ')', ']', '>', ';', '!'])
            .to_string()
    })
}

pub fn format_reply(result: &IngestResult, url: &str) -> String {
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
            format!("Saved: {title}{tags}")
        }
        IngestStatus::Failed { reason } => {
            format!("Failed: {reason}\nURL: {url}")
        }
        IngestStatus::Queued => "Queued for processing.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_youtube_url() {
        let result = classify_url("https://www.youtube.com/watch?v=abc123").expect("valid url");
        assert!(matches!(result, UrlType::YouTube(_)));
    }

    #[test]
    fn test_youtube_short_url() {
        let result = classify_url("https://youtu.be/abc123").expect("valid url");
        assert!(matches!(result, UrlType::YouTube(_)));
    }

    #[test]
    fn test_youtube_music_url() {
        let result = classify_url("https://music.youtube.com/watch?v=abc123").expect("valid url");
        assert!(matches!(result, UrlType::YouTube(_)));
    }

    #[test]
    fn test_article_url() {
        let result = classify_url("https://blog.example.com/post").expect("valid url");
        assert!(matches!(result, UrlType::Article(_)));
    }

    #[test]
    fn test_invalid_url() {
        let result = classify_url("not a url");
        assert!(result.is_err());
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
        use crate::types::{IngestResult, IngestStatus};
        let result = IngestResult {
            status: IngestStatus::Completed,
            note_path: Some("/vault/Inbox/Test.md".to_string()),
            title: Some("Test Article".to_string()),
            tags: vec!["ai".to_string(), "tech".to_string()],
        };
        let reply = format_reply(&result, "https://example.com");
        assert_eq!(reply, "Saved: Test Article\nTags: #ai, #tech");
    }

    #[test]
    fn test_format_reply_completed_no_tags() {
        use crate::types::{IngestResult, IngestStatus};
        let result = IngestResult {
            status: IngestStatus::Completed,
            note_path: None,
            title: Some("Test".to_string()),
            tags: vec![],
        };
        let reply = format_reply(&result, "https://example.com");
        assert_eq!(reply, "Saved: Test");
    }

    #[test]
    fn test_format_reply_failed() {
        use crate::types::{IngestResult, IngestStatus};
        let result = IngestResult {
            status: IngestStatus::Failed {
                reason: "network error".to_string(),
            },
            note_path: None,
            title: None,
            tags: vec![],
        };
        let reply = format_reply(&result, "https://example.com/broken");
        assert_eq!(reply, "Failed: network error\nURL: https://example.com/broken");
    }

    #[test]
    fn test_format_reply_queued() {
        use crate::types::{IngestResult, IngestStatus};
        let result = IngestResult {
            status: IngestStatus::Queued,
            note_path: None,
            title: None,
            tags: vec![],
        };
        let reply = format_reply(&result, "https://example.com");
        assert_eq!(reply, "Queued for processing.");
    }
}
