use crate::types::IngestMethod;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Process-lifetime counter to guarantee uniqueness even for
/// ingests arriving in the same nanosecond (e.g. batch CLI import).
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Generate a trace ID with a method-specific prefix.
///
/// Format: `{prefix}-{6 hex chars}`
///
/// Uniqueness: mixes nanosecond timestamp, process ID, and an atomic
/// counter. No external dependencies (no `rand` crate needed).
pub fn generate(method: IngestMethod) -> String {
    let prefix = method_prefix(method);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    // Mix all three sources, then take lower 24 bits (6 hex chars)
    let mixed = nanos
        .wrapping_mul(6364136223846793005) // LCG multiplier
        ^ (pid as u64) << 16
        ^ seq as u64;
    let hex = format!("{:06x}", mixed & 0x00FF_FFFF);
    format!("{prefix}-{hex}")
}

fn method_prefix(method: IngestMethod) -> &'static str {
    match method {
        IngestMethod::Telegram => "tg",
        IngestMethod::Discord => "dc",
        IngestMethod::Http => "ht",
        IngestMethod::Clipboard => "cb",
        IngestMethod::Cli => "cl",
        IngestMethod::Ntfy => "nf",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_format() {
        let id = generate(IngestMethod::Telegram);
        let re = regex::Regex::new(r"^[a-z]{2}-[0-9a-f]{6}$").expect("valid regex");
        assert!(re.is_match(&id), "trace ID '{id}' does not match expected format");
    }

    #[test]
    fn test_method_prefixes() {
        assert_eq!(method_prefix(IngestMethod::Telegram), "tg");
        assert_eq!(method_prefix(IngestMethod::Discord), "dc");
        assert_eq!(method_prefix(IngestMethod::Http), "ht");
        assert_eq!(method_prefix(IngestMethod::Clipboard), "cb");
        assert_eq!(method_prefix(IngestMethod::Cli), "cl");
        assert_eq!(method_prefix(IngestMethod::Ntfy), "nf");
    }

    #[test]
    fn test_sequential_uniqueness() {
        let id1 = generate(IngestMethod::Cli);
        let id2 = generate(IngestMethod::Cli);
        assert_ne!(id1, id2, "two sequential trace IDs should differ");
    }

    #[test]
    fn test_different_methods_different_prefix() {
        let tg = generate(IngestMethod::Telegram);
        let dc = generate(IngestMethod::Discord);
        assert!(tg.starts_with("tg-"));
        assert!(dc.starts_with("dc-"));
    }
}
