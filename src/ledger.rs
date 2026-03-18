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
    pub domain: Option<String>,
    pub trace_id: Option<String>,
}

const LEDGER_FRONTMATTER: &str = r#"---
title: Borg Ledger
date: {date}
type: system
domain: system
origin: human
tags:
  - obsidian-borg
  - system
---

# Borg Ledger

All URLs ingested by obsidian-borg. This file is machine-maintained - do not edit the table manually.

See also: [[borg-dashboard]]

| Date | Time | Method | Status | Title | Source | Domain | Trace |
|------|------|--------|--------|-------|--------|--------|-------|
"#;

/// Resolve the Borg Ledger path from config.
pub fn ledger_path(config: &Config) -> PathBuf {
    let root = expand_tilde(&config.vault.root_path);
    root.join("system").join("borg-ledger.md")
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
        // Expected: ["", " Date ", " Time ", " Method ", " Status ", " Title ", " Source ", " Domain ", ""]
        if cols.len() < 8 {
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
    let domain_display = entry.domain.as_deref().unwrap_or("-");
    let trace_display = entry.trace_id.as_deref().unwrap_or("-");

    let row = format!(
        "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
        entry.date, entry.time, entry.method, entry.status, title_display, entry.source, domain_display, trace_display,
    );

    use std::io::Write;
    let mut file_ref = &file;
    file_ref
        .write_all(row.as_bytes())
        .context("Failed to write Borg Ledger entry")?;
    file.unlock().ok();

    Ok(())
}

/// Parsed row from the ledger for audit purposes.
#[derive(Debug)]
pub struct ParsedLedgerRow {
    pub date: String,
    pub status: String,
    pub title: String,
    pub source: String,
}

/// Parse all completed entries from the ledger for auditing.
pub fn parse_completed_entries(ledger_path: &Path) -> Result<Vec<ParsedLedgerRow>> {
    if !ledger_path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(ledger_path).context("Failed to read Borg Ledger")?;
    let mut entries = Vec::new();
    for line in content.lines() {
        if !line.starts_with('|') || line.starts_with("| Date") || line.starts_with("|--") {
            continue;
        }
        let cols: Vec<&str> = line.split('|').collect();
        if cols.len() < 8 {
            continue;
        }
        let status = cols[4].trim().to_string();
        if status != "✅" {
            continue;
        }
        let title_raw = cols[5].trim();
        // Strip wiki-link brackets: [[Title]] -> Title
        let title = title_raw
            .strip_prefix("[[")
            .and_then(|s| s.strip_suffix("]]"))
            .unwrap_or(title_raw)
            .to_string();
        entries.push(ParsedLedgerRow {
            date: cols[1].trim().to_string(),
            status,
            title,
            source: cols[6].trim().to_string(),
        });
    }
    Ok(entries)
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
            domain: Some("inbox".to_string()),
            trace_id: None,
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
            domain: None,
            trace_id: None,
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
            domain: None,
            trace_id: None,
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

    #[test]
    fn test_append_entry_with_trace_id() {
        let path = temp_ledger_path().with_file_name("test-trace-id.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-16".to_string(),
            time: "12:01".to_string(),
            method: IngestMethod::Telegram,
            status: LedgerStatus::Completed,
            title: Some("Test Note".to_string()),
            source: "https://example.com".to_string(),
            domain: Some("work".to_string()),
            trace_id: Some("tg-7f3a2c".to_string()),
        };
        append_entry(&path, &entry).expect("append");

        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("| Trace |"), "header should have Trace column");
        assert!(content.contains("tg-7f3a2c"), "row should contain trace ID");

        cleanup(&path);
    }

    #[test]
    fn test_append_entry_without_trace_id() {
        let path = temp_ledger_path().with_file_name("test-no-trace-id.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-16".to_string(),
            time: "12:01".to_string(),
            method: IngestMethod::Cli,
            status: LedgerStatus::Completed,
            title: Some("Test".to_string()),
            source: "https://example.com".to_string(),
            domain: None,
            trace_id: None,
        };
        append_entry(&path, &entry).expect("append");

        let content = fs::read_to_string(&path).expect("read");
        // When no trace_id, the column should show "-"
        assert!(content.contains("| - |"), "missing trace should show dash");

        cleanup(&path);
    }
}
