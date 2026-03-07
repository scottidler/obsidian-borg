use eyre::{Context, Result, bail};
use serde::Deserialize;
use std::process::Command;

use crate::config::FabricConfig;

#[derive(Debug)]
pub struct YouTubeContent {
    pub title: String,
    pub channel: String,
    pub duration_secs: f64,
    pub published_at: String,
    pub transcript: String,
    pub video_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ClassificationResult {
    pub folder: String,
    pub confidence: f64,
    #[serde(default)]
    pub suggested_tags: Vec<String>,
}

pub async fn run_pattern(pattern: &str, input: &str, config: &FabricConfig) -> Result<String> {
    let truncated = truncate_input(input, config.max_content_chars);
    let binary = resolve_binary(config);

    let mut cmd = Command::new(&binary);
    cmd.args(["-p", pattern]);
    if !config.model.is_empty() {
        cmd.args(["-m", &config.model]);
    }
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().context("Failed to spawn fabric binary")?;

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(truncated.as_bytes())
            .context("Failed to write to fabric stdin")?;
    }

    let output = child.wait_with_output().context("Failed to wait for fabric")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("fabric -p {pattern} failed: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub async fn fetch_youtube(url: &str, config: &FabricConfig) -> Result<YouTubeContent> {
    // Get metadata via fabric -y <url> --metadata
    let binary = resolve_binary(config);
    log::debug!("fabric: fetching YouTube metadata for {url}");
    let mut cmd = Command::new(&binary);
    cmd.args(["-y", url, "--metadata"]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.spawn()?.wait_with_output()?;
    let metadata_json = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else {
        String::new()
    };

    // Parse metadata
    let (title, channel, duration_secs, published_at, video_id) = parse_youtube_metadata(&metadata_json, url);

    // Get transcript via fabric -y <url> --transcript
    log::debug!("fabric: fetching YouTube transcript for {url}");
    let mut cmd = Command::new(&binary);
    cmd.args(["-y", url, "--transcript"]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.spawn()?.wait_with_output()?;
    let transcript = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::warn!("fabric -y --transcript failed: {stderr}");
        String::new()
    };

    Ok(YouTubeContent {
        title,
        channel,
        duration_secs,
        published_at,
        transcript,
        video_id,
    })
}

pub async fn fetch_article(url: &str, config: &FabricConfig) -> Result<String> {
    // Primary: fabric -u <url>
    let binary = resolve_binary(config);
    log::debug!("fabric: fetching article for {url}");
    let mut cmd = Command::new(&binary);
    cmd.args(["-u", url]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.spawn()?.wait_with_output()?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() {
            return Ok(text);
        }
    }

    // Fallback: markitdown
    log::debug!("fabric -u failed, trying markitdown-cli for {url}");
    let output = Command::new("markitdown-cli")
        .arg(url)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|c| c.wait_with_output());

    if let Ok(output) = output
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !text.is_empty() {
            return Ok(text);
        }
    }

    // Last resort: jina.rs (caller handles this)
    bail!("Both fabric -u and markitdown-cli failed for {url}")
}

pub async fn summarize(content: &str, is_youtube: bool, config: &FabricConfig) -> Result<String> {
    let pattern = if is_youtube {
        &config.summarize_pattern_youtube
    } else {
        &config.summarize_pattern_article
    };
    run_pattern(pattern, content, config).await
}

pub async fn generate_tags(content: &str, config: &FabricConfig) -> Result<Vec<String>> {
    let output = run_pattern(&config.tag_pattern, content, config).await?;
    let tags: Vec<String> = output
        .split_whitespace()
        .map(|t| t.trim_matches('#').to_string())
        .filter(|t| !t.is_empty())
        .collect();
    Ok(tags)
}

pub async fn classify_topic(title: &str, summary: &str, config: &FabricConfig) -> Result<ClassificationResult> {
    let input = format!("Title: {title}\n\nSummary:\n{summary}");
    let output = run_pattern(&config.classify_pattern, &input, config).await?;

    // Try to parse JSON from the output (fabric may wrap it in markdown)
    let json_str = extract_json(&output);
    let result: ClassificationResult =
        serde_json::from_str(&json_str).context("Failed to parse classification JSON")?;
    Ok(result)
}

fn extract_json(text: &str) -> String {
    // Look for JSON object in output (may be wrapped in ```json blocks)
    let trimmed = text.trim();
    if trimmed.starts_with('{') {
        return trimmed.to_string();
    }
    // Try extracting from markdown code block
    if let Some(start) = trimmed.find('{')
        && let Some(end) = trimmed.rfind('}')
    {
        return trimmed[start..=end].to_string();
    }
    trimmed.to_string()
}

fn parse_youtube_metadata(json_str: &str, url: &str) -> (String, String, f64, String, String) {
    let video_id = crate::youtube::extract_video_id(url).unwrap_or_default();

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(json_str) {
        let title = json["title"].as_str().unwrap_or("Unknown").to_string();
        let channel = json["channel"]
            .as_str()
            .or_else(|| json["uploader"].as_str())
            .unwrap_or("Unknown")
            .to_string();
        let duration = json["duration"].as_f64().unwrap_or(0.0);
        let published = json["upload_date"]
            .as_str()
            .or_else(|| json["published_at"].as_str())
            .unwrap_or("")
            .to_string();
        (title, channel, duration, published, video_id)
    } else {
        (
            "Unknown".to_string(),
            "Unknown".to_string(),
            0.0,
            String::new(),
            video_id,
        )
    }
}

fn truncate_input(input: &str, max_chars: usize) -> String {
    if max_chars == 0 || input.len() <= max_chars {
        input.to_string()
    } else {
        input[..max_chars].to_string()
    }
}

/// Resolve the fabric binary path — if not absolute, try `which` to find it.
pub fn resolve_binary(config: &FabricConfig) -> String {
    let binary = &config.binary;
    if binary.starts_with('/') || binary.starts_with("./") {
        return binary.clone();
    }
    // Try `which` to resolve from shell PATH (covers ~/go/bin etc.)
    if let Ok(output) = Command::new("which")
        .arg(binary)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
    {
        if output.status.success() {
            let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !resolved.is_empty() {
                log::debug!("Resolved fabric binary: {binary} -> {resolved}");
                return resolved;
            }
        }
    }
    binary.clone()
}

pub fn is_available(config: &FabricConfig) -> bool {
    let binary = resolve_binary(config);
    Command::new(&binary)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_json_bare() {
        let input = r#"{"folder": "Tech", "confidence": 0.9, "suggested_tags": []}"#;
        let result = extract_json(input);
        assert!(result.starts_with('{'));
        let parsed: ClassificationResult = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed.folder, "Tech");
    }

    #[test]
    fn test_extract_json_markdown_wrapped() {
        let input = "```json\n{\"folder\": \"Tech\", \"confidence\": 0.8}\n```";
        let result = extract_json(input);
        let parsed: ClassificationResult = serde_json::from_str(&result).expect("valid json");
        assert_eq!(parsed.folder, "Tech");
    }

    #[test]
    fn test_truncate_input() {
        assert_eq!(truncate_input("hello world", 5), "hello");
        assert_eq!(truncate_input("hello", 10), "hello");
        assert_eq!(truncate_input("hello", 0), "hello");
    }

    #[test]
    fn test_parse_youtube_metadata_valid() {
        let json = r#"{"title": "Test Video", "channel": "TestChan", "duration": 120.0, "upload_date": "2026-01-01"}"#;
        let (title, channel, dur, published, _vid) = parse_youtube_metadata(json, "https://youtube.com/watch?v=abc123");
        assert_eq!(title, "Test Video");
        assert_eq!(channel, "TestChan");
        assert!((dur - 120.0).abs() < f64::EPSILON);
        assert_eq!(published, "2026-01-01");
    }

    #[test]
    fn test_parse_youtube_metadata_invalid() {
        let (title, channel, dur, _, _) = parse_youtube_metadata("not json", "https://youtube.com/watch?v=abc");
        assert_eq!(title, "Unknown");
        assert_eq!(channel, "Unknown");
        assert!((dur - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_classification_result_deserialize() {
        let json = r#"{"folder": "Tech/AI-LLM", "confidence": 0.85, "reasoning": "AI content", "suggested_tags": ["ai", "llm"]}"#;
        let result: ClassificationResult = serde_json::from_str(json).expect("valid");
        assert_eq!(result.folder, "Tech/AI-LLM");
        assert!((result.confidence - 0.85).abs() < f64::EPSILON);
        assert_eq!(result.suggested_tags, vec!["ai", "llm"]);
    }
}
