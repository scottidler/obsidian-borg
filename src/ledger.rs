use crate::config::Config;
use crate::types::IngestMethod;
use eyre::{Context, Result};
use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerStatus {
    Completed,
    Failed,
    Skipped,
}

impl std::fmt::Display for LedgerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => write!(f, "✅"),
            Self::Failed => write!(f, "❌"),
            Self::Skipped => write!(f, "⏭️"),
        }
    }
}

pub struct LedgerEntry {
    pub date: String,
    pub time: String,
    pub method: IngestMethod,
    pub status: LedgerStatus,
    pub title: Option<String>,
    pub source: String,
    pub original: String,
    pub folder: Option<String>,
}

const LEDGER_FRONTMATTER: &str = r#"---
date: {date}
type: system
tags:
  - obsidian-borg
  - system
---

# Borg Ledger

All URLs ingested by obsidian-borg. This file is machine-maintained — do not edit the table manually.

| Date | Time | Method | Status | Title | Source | Original | Folder |
|------|------|--------|--------|-------|--------|----------|--------|
"#;

/// Resolve the Borg Ledger path from config.
pub fn ledger_path(config: &Config) -> PathBuf {
    let root = expand_tilde(&config.vault.root_path);
    root.join("⚙️ System").join("borg-ledger.md")
}

/// Create the Borg Ledger file with frontmatter and header if it doesn't exist.
pub fn ensure_ledger_exists(ledger_path: &Path) -> Result<()> {
    if ledger_path.exists() {
        return Ok(());
    }
    if let Some(parent) = ledger_path.parent() {
        fs::create_dir_all(parent).context("Failed to create Borg Ledger directory")?;
    }
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let content = LEDGER_FRONTMATTER.replace("{date}", &date);
    fs::write(ledger_path, content).context("Failed to create Borg Ledger")?;
    log::info!("Created Borg Ledger at {}", ledger_path.display());
    Ok(())
}

/// Check if canonical URL exists in log with a ✅ status. Returns the date if found.
pub fn check_duplicate(ledger_path: &Path, canonical_url: &str) -> Result<Option<String>> {
    if !ledger_path.exists() {
        return Ok(None);
    }

    let file = OpenOptions::new()
        .read(true)
        .open(ledger_path)
        .context("Failed to open Borg Ledger for reading")?;
    file.lock_shared()
        .context("Failed to acquire shared lock on Borg Ledger")?;

    let content = fs::read_to_string(ledger_path).context("Failed to read Borg Ledger")?;
    file.unlock().ok();

    for line in content.lines() {
        if !line.starts_with('|') || line.starts_with("| Date") || line.starts_with("|--") {
            continue;
        }
        let cols: Vec<&str> = line.split('|').collect();
        // Expected: ["", " Date ", " Time ", " Method ", " Status ", " Title ", " Source ", " Original ", " Folder ", ""]
        if cols.len() < 9 {
            continue;
        }
        let status = cols[4].trim();
        let source = cols[6].trim();
        if status == "✅" && source == canonical_url {
            return Ok(Some(cols[1].trim().to_string()));
        }
    }

    Ok(None)
}

/// Append a row to the Borg Ledger table.
pub fn append_entry(ledger_path: &Path, entry: &LedgerEntry) -> Result<()> {
    ensure_ledger_exists(ledger_path)?;

    let file = OpenOptions::new()
        .append(true)
        .open(ledger_path)
        .context("Failed to open Borg Ledger for appending")?;
    file.lock_exclusive()
        .context("Failed to acquire exclusive lock on Borg Ledger")?;

    let title_display = entry
        .title
        .as_ref()
        .map(|t| format!("[[{}]]", t))
        .unwrap_or_else(|| "—".to_string());
    let folder_display = entry.folder.as_deref().unwrap_or("—");

    let row = format!(
        "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
        entry.date, entry.time, entry.method, entry.status, title_display, entry.source, entry.original, folder_display,
    );

    use std::io::Write;
    let mut file_ref = &file;
    file_ref
        .write_all(row.as_bytes())
        .context("Failed to write Borg Ledger entry")?;
    file.unlock().ok();

    Ok(())
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
    use std::fs;

    fn temp_ledger_path() -> PathBuf {
        let dir = std::env::temp_dir().join("obsidian-borg-test-ledger");
        fs::create_dir_all(&dir).ok();
        dir.join("borg-ledger.md")
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_ensure_ledger_exists_creates_file() {
        let path = temp_ledger_path().with_file_name("test-create.md");
        cleanup(&path);
        ensure_ledger_exists(&path).expect("should create");
        assert!(path.exists());
        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("# Borg Ledger"));
        assert!(content.contains("| Date |"));
        cleanup(&path);
    }

    #[test]
    fn test_ensure_ledger_exists_idempotent() {
        let path = temp_ledger_path().with_file_name("test-idempotent.md");
        cleanup(&path);
        ensure_ledger_exists(&path).expect("first");
        let content1 = fs::read_to_string(&path).expect("read");
        ensure_ledger_exists(&path).expect("second");
        let content2 = fs::read_to_string(&path).expect("read");
        assert_eq!(content1, content2);
        cleanup(&path);
    }

    #[test]
    fn test_check_duplicate_empty_log() {
        let path = temp_ledger_path().with_file_name("test-dedup-empty.md");
        cleanup(&path);
        ensure_ledger_exists(&path).expect("create");
        let result = check_duplicate(&path, "https://example.com").expect("check");
        assert!(result.is_none());
        cleanup(&path);
    }

    #[test]
    fn test_check_duplicate_no_file() {
        let path = temp_ledger_path().with_file_name("nonexistent.md");
        let result = check_duplicate(&path, "https://example.com").expect("check");
        assert!(result.is_none());
    }

    #[test]
    fn test_append_and_check_duplicate() {
        let path = temp_ledger_path().with_file_name("test-append-dedup.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-07".to_string(),
            time: "14:30".to_string(),
            method: IngestMethod::Cli,
            status: LedgerStatus::Completed,
            title: Some("Test Article".to_string()),
            source: "https://example.com/article".to_string(),
            original: "https://example.com/article?utm_source=x".to_string(),
            folder: Some("📥 Inbox".to_string()),
        };
        append_entry(&path, &entry).expect("append");

        // Should find duplicate
        let result = check_duplicate(&path, "https://example.com/article").expect("check");
        assert_eq!(result, Some("2026-03-07".to_string()));

        // Different URL should not be duplicate
        let result = check_duplicate(&path, "https://example.com/other").expect("check");
        assert!(result.is_none());

        cleanup(&path);
    }

    #[test]
    fn test_failed_entry_not_duplicate() {
        let path = temp_ledger_path().with_file_name("test-failed-not-dup.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-07".to_string(),
            time: "14:30".to_string(),
            method: IngestMethod::Telegram,
            status: LedgerStatus::Failed,
            title: None,
            source: "https://example.com/broken".to_string(),
            original: "https://example.com/broken".to_string(),
            folder: None,
        };
        append_entry(&path, &entry).expect("append");

        // Failed entries should NOT count as duplicates
        let result = check_duplicate(&path, "https://example.com/broken").expect("check");
        assert!(result.is_none());

        cleanup(&path);
    }

    #[test]
    fn test_skipped_entry_not_duplicate() {
        let path = temp_ledger_path().with_file_name("test-skipped-not-dup.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-07".to_string(),
            time: "14:30".to_string(),
            method: IngestMethod::Clipboard,
            status: LedgerStatus::Skipped,
            title: None,
            source: "https://example.com/dup".to_string(),
            original: "https://example.com/dup".to_string(),
            folder: None,
        };
        append_entry(&path, &entry).expect("append");

        // Skipped entries should NOT count as duplicates
        let result = check_duplicate(&path, "https://example.com/dup").expect("check");
        assert!(result.is_none());

        cleanup(&path);
    }

    #[test]
    fn test_ledger_status_display() {
        assert_eq!(format!("{}", LedgerStatus::Completed), "✅");
        assert_eq!(format!("{}", LedgerStatus::Failed), "❌");
        assert_eq!(format!("{}", LedgerStatus::Skipped), "⏭️");
    }
}
