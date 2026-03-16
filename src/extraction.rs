use eyre::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Extract markdown text from a file using markitdown-cli.
///
/// Returns the extracted markdown content, or an error if the tool
/// is not found or extraction fails.
pub fn extract_markdown(file_path: &Path) -> Result<String> {
    let output = Command::new("markitdown-cli")
        .arg(file_path.as_os_str())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("markitdown-cli not found - install with: pipx install markitdown-cli")?
        .wait_with_output()
        .context("Failed to wait for markitdown-cli")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eyre::bail!("markitdown-cli failed for {}: {stderr}", file_path.display());
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        eyre::bail!("markitdown-cli produced no output for {}", file_path.display());
    }

    Ok(text)
}

/// Check if markitdown-cli is available on PATH.
pub fn is_available() -> bool {
    Command::new("markitdown-cli")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_extract_markdown_nonexistent_file() {
        let path = Path::new("/tmp/obsidian-borg-test-nonexistent-file.pdf");
        // Should fail (file doesn't exist or markitdown-cli not installed)
        let result = extract_markdown(path);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_markdown_plain_text_file() {
        let tmp_dir = std::env::temp_dir().join("obsidian-borg-test-extraction");
        let _ = fs::create_dir_all(&tmp_dir);
        let test_file = tmp_dir.join("test.txt");
        fs::write(&test_file, "Hello, this is a test document with some content.").expect("write");

        let result = extract_markdown(&test_file);
        // If markitdown-cli is installed, it should extract text; otherwise error is fine
        if let Ok(text) = result {
            assert!(!text.is_empty());
        }

        let _ = fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn test_is_available() {
        // Just ensure it doesn't panic - result depends on environment
        let _ = is_available();
    }
}
