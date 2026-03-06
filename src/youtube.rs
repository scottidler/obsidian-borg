use eyre::{Context, Result, bail};
use std::process::Command;

#[derive(Debug)]
pub struct VideoMetadata {
    pub title: String,
    pub uploader: String,
    pub duration_secs: f64,
}

pub fn fetch_metadata(url: &str) -> Result<VideoMetadata> {
    let output = Command::new("yt-dlp")
        .args(["--dump-json", "--no-download", "--no-warnings", url])
        .output()
        .context("Failed to run yt-dlp - is it installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp failed: {stderr}");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).context("Failed to parse yt-dlp JSON")?;

    Ok(VideoMetadata {
        title: json["title"].as_str().unwrap_or("Unknown").to_string(),
        uploader: json["uploader"].as_str().unwrap_or("Unknown").to_string(),
        duration_secs: json["duration"].as_f64().unwrap_or(0.0),
    })
}

pub fn fetch_subtitles(url: &str) -> Result<Option<String>> {
    let output = Command::new("yt-dlp")
        .args([
            "--write-auto-sub",
            "--sub-lang",
            "en",
            "--sub-format",
            "vtt",
            "--skip-download",
            "--print",
            "%(requested_subtitles)j",
            url,
        ])
        .output()
        .context("Failed to run yt-dlp for subtitles")?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();

    if trimmed == "NA" || trimmed == "null" || trimmed.is_empty() {
        return Ok(None);
    }

    // Try to get the subtitle file path from the JSON
    let subs: serde_json::Value = serde_json::from_str(trimmed).unwrap_or_default();
    if let Some(en_sub) = subs.get("en")
        && let Some(filepath) = en_sub.get("filepath").and_then(|f| f.as_str())
    {
        let content = std::fs::read_to_string(filepath).context("Failed to read subtitle file")?;
        let cleaned = clean_vtt(&content);
        let _ = std::fs::remove_file(filepath);
        return Ok(Some(cleaned));
    }

    Ok(None)
}

pub fn extract_audio(url: &str, output_dir: &str) -> Result<String> {
    let output_template = format!("{output_dir}/%(id)s.%(ext)s");

    let output = Command::new("yt-dlp")
        .args([
            "-x",
            "--audio-format",
            "mp3",
            "--audio-quality",
            "5",
            "-o",
            &output_template,
            url,
        ])
        .output()
        .context("Failed to run yt-dlp for audio extraction")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("yt-dlp audio extraction failed: {stderr}");
    }

    // Find the output file
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if line.contains("[ExtractAudio] Destination:")
            && let Some(path) = line.split("Destination:").nth(1)
        {
            return Ok(path.trim().to_string());
        }
    }

    bail!("Could not determine audio output path from yt-dlp")
}

fn clean_vtt(vtt: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut last_line = String::new();

    for line in vtt.lines() {
        let line = line.trim();

        // Skip VTT headers and timestamps
        if line.starts_with("WEBVTT")
            || line.starts_with("Kind:")
            || line.starts_with("Language:")
            || line.contains("-->")
            || line.is_empty()
        {
            continue;
        }

        // Skip numeric cue identifiers
        if line.parse::<u32>().is_ok() {
            continue;
        }

        // Remove HTML tags
        let cleaned = line
            .replace("<c>", "")
            .replace("</c>", "")
            .replace("<i>", "")
            .replace("</i>", "");

        // Deduplicate consecutive identical lines
        if cleaned != last_line {
            lines.push(cleaned.clone());
            last_line = cleaned;
        }
    }

    lines.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_vtt_removes_headers() {
        let vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n00:00:00.000 --> 00:00:05.000\nHello world\n\n00:00:05.000 --> 00:00:10.000\nThis is a test";
        let result = clean_vtt(vtt);
        assert_eq!(result, "Hello world This is a test");
    }

    #[test]
    fn test_clean_vtt_removes_html_tags() {
        let vtt = "00:00:00.000 --> 00:00:05.000\n<c>Hello</c> <i>world</i>";
        let result = clean_vtt(vtt);
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_clean_vtt_deduplicates() {
        let vtt = "00:00:00.000 --> 00:00:05.000\nHello\n\n00:00:05.000 --> 00:00:10.000\nHello\n\n00:00:10.000 --> 00:00:15.000\nWorld";
        let result = clean_vtt(vtt);
        assert_eq!(result, "Hello World");
    }
}
