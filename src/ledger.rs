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
    Replaced,
}

impl std::fmt::Display for LedgerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => write!(f, "✅"),
            Self::Failed => write!(f, "❌"),
            Self::Skipped => write!(f, "⏭️"),
            Self::Replaced => write!(f, "🔄"),
        }
    }
}

pub struct LedgerEntry {
    pub date: String,
    pub time: String,
    pub method: IngestMethod,
    pub status: LedgerStatus,
    pub title: Option<String>,
    pub path: Option<String>,
    pub source: String,
    pub domain: Option<String>,
    pub trace_id: Option<String>,
}

const LEDGER_FRONTMATTER: &str = r#"---
title: Borg Ledger
date: {date}
type: system
domain: system
origin: authored
tags:
  - obsidian-borg
  - system
---

# Borg Ledger

All URLs ingested by obsidian-borg. This file is machine-maintained - do not edit the table manually.

See also: [[borg-dashboard]]

| Date | Time | Method | Status | Title | Path | Source | Domain | Trace |
|------|------|--------|--------|-------|------|--------|--------|-------|
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
        // New format (9 data cols): ["", Date, Time, Method, Status, Title, Path, Source, Domain, Trace, ""]
        // Old format (8 data cols): ["", Date, Time, Method, Status, Title, Source, Domain, Trace, ""]
        if cols.len() < 8 {
            continue;
        }
        let status = cols[4].trim();
        // Source is at index 7 (new format, 10+ cols) or index 6 (old format)
        let source = if cols.len() >= 11 { cols[7].trim() } else { cols[6].trim() };
        if status == "✅" && source == canonical_url {
            return Ok(Some(cols[1].trim().to_string()));
        }
    }

    Ok(None)
}

/// Result from finding a completed entry for a content key.
#[derive(Debug)]
pub struct CompletedEntry {
    pub date: String,
    pub path: String,
    pub line_number: usize,
}

/// Find the most recent completed entry for a content key (canonical URL or normalized text).
/// Returns the vault-relative path and line number for replacement.
pub fn find_completed(ledger_path: &Path, content_key: &str) -> Result<Option<CompletedEntry>> {
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

    let mut last_match: Option<CompletedEntry> = None;

    for (line_number, line) in content.lines().enumerate() {
        if !line.starts_with('|') || line.starts_with("| Date") || line.starts_with("|--") {
            continue;
        }
        let cols: Vec<&str> = line.split('|').collect();
        if cols.len() < 8 {
            continue;
        }
        let status = cols[4].trim();
        // New format (11+ cols): source at index 7, path at index 6
        // Old format (10 cols): source at index 6, no path
        let (source, path) = if cols.len() >= 11 {
            (cols[7].trim(), cols[6].trim().to_string())
        } else {
            (cols[6].trim(), "-".to_string())
        };
        if status == "✅" && source == content_key {
            last_match = Some(CompletedEntry {
                date: cols[1].trim().to_string(),
                path,
                line_number,
            });
        }
    }

    Ok(last_match)
}

/// Mark an existing ledger row as replaced (✅ -> 🔄).
/// Reads the entire file, replaces the status in the target line, and writes back.
pub fn mark_replaced(ledger_path: &Path, line_number: usize) -> Result<()> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(ledger_path)
        .context("Failed to open Borg Ledger for update")?;
    file.lock_exclusive()
        .context("Failed to acquire exclusive lock on Borg Ledger")?;

    let content = fs::read_to_string(ledger_path).context("Failed to read Borg Ledger")?;
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();

    if line_number < lines.len() {
        // Replace the first ✅ in the line with 🔄
        lines[line_number] = lines[line_number].replacen("✅", "🔄", 1);
    }

    let new_content = lines.join("\n");
    // Preserve trailing newline if original had one
    let final_content = if content.ends_with('\n') { format!("{new_content}\n") } else { new_content };

    fs::write(ledger_path, final_content).context("Failed to write updated Borg Ledger")?;
    file.unlock().ok();

    Ok(())
}

/// Filter criteria for querying ledger entries.
#[derive(Debug, Default)]
pub struct EntryFilter {
    pub source: Option<String>,
    pub domain: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
}

/// Extended completed entry with all fields for reingest.
#[derive(Debug)]
pub struct QueriedEntry {
    pub date: String,
    pub method: String,
    pub title: String,
    pub path: String,
    pub source: String,
    pub domain: String,
    pub line_number: usize,
}

/// Query all completed entries from the ledger, applying optional filters.
pub fn query_entries(ledger_path: &Path, filter: &EntryFilter) -> Result<Vec<QueriedEntry>> {
    if !ledger_path.exists() {
        return Ok(Vec::new());
    }

    let file = OpenOptions::new()
        .read(true)
        .open(ledger_path)
        .context("Failed to open Borg Ledger for reading")?;
    file.lock_shared()
        .context("Failed to acquire shared lock on Borg Ledger")?;

    let content = fs::read_to_string(ledger_path).context("Failed to read Borg Ledger")?;
    file.unlock().ok();

    let mut entries = Vec::new();

    for (line_number, line) in content.lines().enumerate() {
        if !line.starts_with('|') || line.starts_with("| Date") || line.starts_with("|--") {
            continue;
        }
        let cols: Vec<&str> = line.split('|').collect();
        if cols.len() < 8 {
            continue;
        }
        let status = cols[4].trim();
        if status != "✅" {
            continue;
        }

        let date = cols[1].trim().to_string();
        let method = cols[3].trim().to_string();
        let title_raw = cols[5].trim();
        let title = title_raw
            .strip_prefix("[[")
            .and_then(|s| s.strip_suffix("]]"))
            .unwrap_or(title_raw)
            .to_string();

        // Handle old (10 cols) vs new (11+ cols) format
        let (path, source, domain) = if cols.len() >= 11 {
            (
                cols[6].trim().to_string(),
                cols[7].trim().to_string(),
                cols[8].trim().to_string(),
            )
        } else {
            ("-".to_string(), cols[6].trim().to_string(), cols[7].trim().to_string())
        };

        // Apply filters
        if let Some(ref f_source) = filter.source
            && source != *f_source
        {
            continue;
        }
        if let Some(ref f_domain) = filter.domain
            && domain != *f_domain
        {
            continue;
        }
        if let Some(ref f_before) = filter.before
            && date.as_str() >= f_before.as_str()
        {
            continue;
        }
        if let Some(ref f_after) = filter.after
            && date.as_str() <= f_after.as_str()
        {
            continue;
        }

        entries.push(QueriedEntry {
            date,
            method,
            title,
            path,
            source,
            domain,
            line_number,
        });
    }

    Ok(entries)
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
        .unwrap_or_else(|| "-".to_string());
    let path_display = entry.path.as_deref().unwrap_or("-");
    let domain_display = entry.domain.as_deref().unwrap_or("-");
    let trace_display = entry.trace_id.as_deref().unwrap_or("-");

    let row = format!(
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
        entry.date,
        entry.time,
        entry.method,
        entry.status,
        title_display,
        path_display,
        entry.source,
        domain_display,
        trace_display,
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
        // Source is at index 7 (new format, 10+ cols) or index 6 (old format)
        let source = if cols.len() >= 11 { cols[7].trim() } else { cols[6].trim() };
        entries.push(ParsedLedgerRow {
            date: cols[1].trim().to_string(),
            status,
            title,
            source: source.to_string(),
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
            path: None,
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
            path: None,
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
            path: None,
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
            path: None,
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
            path: None,
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

    #[test]
    fn test_append_entry_with_path() {
        let path = temp_ledger_path().with_file_name("test-with-path.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-18".to_string(),
            time: "10:00".to_string(),
            method: IngestMethod::Cli,
            status: LedgerStatus::Completed,
            title: Some("Test Video".to_string()),
            path: Some("notes/test-video.md".to_string()),
            source: "https://www.youtube.com/watch?v=abc".to_string(),
            domain: Some("ai".to_string()),
            trace_id: Some("cl-abc123".to_string()),
        };
        append_entry(&path, &entry).expect("append");

        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("| Path |"), "header should have Path column");
        assert!(
            content.contains("notes/test-video.md"),
            "row should contain vault-relative path"
        );

        // Dedup should still work with the new format
        let result = check_duplicate(&path, "https://www.youtube.com/watch?v=abc").expect("check");
        assert_eq!(result, Some("2026-03-18".to_string()));

        cleanup(&path);
    }

    #[test]
    fn test_check_duplicate_backward_compat_old_format() {
        let path = temp_ledger_path().with_file_name("test-old-format.md");
        cleanup(&path);

        // Write a ledger with the OLD format (no Path column)
        let old_header = "---\ntitle: Borg Ledger\ndate: 2026-03-01\n---\n\n\
            | Date | Time | Method | Status | Title | Source | Domain | Trace |\n\
            |------|------|--------|--------|-------|--------|--------|-------|\n\
            | 2026-03-01 | 12:00 | cli | \u{2705} | [[Old Note]] | https://example.com/old | ai | cl-111111 |\n";
        fs::write(&path, old_header).expect("write old format");

        // Should find duplicates in old-format ledger
        let result = check_duplicate(&path, "https://example.com/old").expect("check");
        assert_eq!(result, Some("2026-03-01".to_string()));

        // Non-matching should return None
        let result = check_duplicate(&path, "https://example.com/other").expect("check");
        assert!(result.is_none());

        cleanup(&path);
    }

    #[test]
    fn test_find_completed_returns_path() {
        let path = temp_ledger_path().with_file_name("test-find-completed.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-18".to_string(),
            time: "10:00".to_string(),
            method: IngestMethod::Cli,
            status: LedgerStatus::Completed,
            title: Some("Test Note".to_string()),
            path: Some("notes/test-note.md".to_string()),
            source: "https://example.com/article".to_string(),
            domain: Some("ai".to_string()),
            trace_id: None,
        };
        append_entry(&path, &entry).expect("append");

        let result = find_completed(&path, "https://example.com/article").expect("find");
        assert!(result.is_some());
        let entry = result.expect("should have entry");
        assert_eq!(entry.date, "2026-03-18");
        assert_eq!(entry.path, "notes/test-note.md");

        // Non-matching key should return None
        let result = find_completed(&path, "https://example.com/other").expect("find");
        assert!(result.is_none());

        cleanup(&path);
    }

    #[test]
    fn test_mark_replaced_changes_status() {
        let path = temp_ledger_path().with_file_name("test-mark-replaced.md");
        cleanup(&path);

        let entry = LedgerEntry {
            date: "2026-03-18".to_string(),
            time: "10:00".to_string(),
            method: IngestMethod::Cli,
            status: LedgerStatus::Completed,
            title: Some("Test Note".to_string()),
            path: Some("notes/test-note.md".to_string()),
            source: "https://example.com/article".to_string(),
            domain: Some("ai".to_string()),
            trace_id: None,
        };
        append_entry(&path, &entry).expect("append");

        // Find the entry and get its line number
        let existing = find_completed(&path, "https://example.com/article")
            .expect("find")
            .expect("should exist");

        // Mark it as replaced
        mark_replaced(&path, existing.line_number).expect("mark");

        // Now check_duplicate should NOT find it (only ✅ counts)
        let result = check_duplicate(&path, "https://example.com/article").expect("check");
        assert!(result.is_none(), "replaced entry should not count as duplicate");

        // find_completed should also NOT find it
        let result = find_completed(&path, "https://example.com/article").expect("find");
        assert!(result.is_none(), "replaced entry should not be found");

        // Verify the file contains 🔄
        let content = fs::read_to_string(&path).expect("read");
        assert!(content.contains("🔄"), "row should have replaced status");

        cleanup(&path);
    }

    #[test]
    fn test_replaced_status_display() {
        assert_eq!(format!("{}", LedgerStatus::Replaced), "🔄");
    }

    #[test]
    fn test_query_entries_no_filter() {
        let path = temp_ledger_path().with_file_name("test-query-no-filter.md");
        cleanup(&path);

        for i in 1..=3 {
            let entry = LedgerEntry {
                date: format!("2026-03-{:02}", i),
                time: "10:00".to_string(),
                method: IngestMethod::Cli,
                status: LedgerStatus::Completed,
                title: Some(format!("Note {i}")),
                path: Some(format!("notes/note-{i}.md")),
                source: format!("https://example.com/{i}"),
                domain: Some(if i <= 2 { "ai" } else { "tech" }.to_string()),
                trace_id: None,
            };
            append_entry(&path, &entry).expect("append");
        }

        let filter = EntryFilter::default();
        let entries = query_entries(&path, &filter).expect("query");
        assert_eq!(entries.len(), 3);

        cleanup(&path);
    }

    #[test]
    fn test_query_entries_domain_filter() {
        let path = temp_ledger_path().with_file_name("test-query-domain.md");
        cleanup(&path);

        for (i, domain) in [("ai", 1), ("ai", 2), ("tech", 3)] {
            let entry = LedgerEntry {
                date: format!("2026-03-{:02}", domain),
                time: "10:00".to_string(),
                method: IngestMethod::Cli,
                status: LedgerStatus::Completed,
                title: Some(format!("Note {}", domain)),
                path: Some(format!("notes/note-{}.md", domain)),
                source: format!("https://example.com/{}", domain),
                domain: Some(i.to_string()),
                trace_id: None,
            };
            append_entry(&path, &entry).expect("append");
        }

        let filter = EntryFilter {
            domain: Some("ai".to_string()),
            ..Default::default()
        };
        let entries = query_entries(&path, &filter).expect("query");
        assert_eq!(entries.len(), 2);

        cleanup(&path);
    }

    #[test]
    fn test_query_entries_date_filter() {
        let path = temp_ledger_path().with_file_name("test-query-date.md");
        cleanup(&path);

        for i in 1..=5 {
            let entry = LedgerEntry {
                date: format!("2026-03-{:02}", i),
                time: "10:00".to_string(),
                method: IngestMethod::Cli,
                status: LedgerStatus::Completed,
                title: Some(format!("Note {i}")),
                path: Some(format!("notes/note-{i}.md")),
                source: format!("https://example.com/{i}"),
                domain: Some("ai".to_string()),
                trace_id: None,
            };
            append_entry(&path, &entry).expect("append");
        }

        // After 2026-03-02 should get entries 03, 04, 05
        let filter = EntryFilter {
            after: Some("2026-03-02".to_string()),
            ..Default::default()
        };
        let entries = query_entries(&path, &filter).expect("query");
        assert_eq!(entries.len(), 3);

        // Before 2026-03-04 should get entries 01, 02, 03
        let filter = EntryFilter {
            before: Some("2026-03-04".to_string()),
            ..Default::default()
        };
        let entries = query_entries(&path, &filter).expect("query");
        assert_eq!(entries.len(), 3);

        cleanup(&path);
    }

    #[test]
    fn test_query_entries_source_filter() {
        let path = temp_ledger_path().with_file_name("test-query-source.md");
        cleanup(&path);

        for i in 1..=3 {
            let entry = LedgerEntry {
                date: format!("2026-03-{:02}", i),
                time: "10:00".to_string(),
                method: IngestMethod::Cli,
                status: LedgerStatus::Completed,
                title: Some(format!("Note {i}")),
                path: Some(format!("notes/note-{i}.md")),
                source: format!("https://example.com/{i}"),
                domain: Some("ai".to_string()),
                trace_id: None,
            };
            append_entry(&path, &entry).expect("append");
        }

        let filter = EntryFilter {
            source: Some("https://example.com/2".to_string()),
            ..Default::default()
        };
        let entries = query_entries(&path, &filter).expect("query");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "https://example.com/2");

        cleanup(&path);
    }
}
