use crate::config::CanonicalRule;
use url::Url;

const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "utm_id",
    "fbclid",
    "gclid",
    "gclsrc",
    "dclid",
    "gbraid",
    "wbraid",
    "msclkid",
    "twclid",
    "li_fat_id",
    "mc_cid",
    "mc_eid",
    "oly_anon_id",
    "oly_enc_id",
    "_openstat",
    "vero_id",
    "wickedid",
    "yclid",
    "hsa_cam",
    "hsa_grp",
    "hsa_mt",
    "hsa_src",
    "hsa_ad",
    "hsa_acc",
    "hsa_net",
    "hsa_ver",
    "hsa_la",
    "hsa_ol",
    "hsa_kw",
    "hsa_tgt",
    "ref",
    "ref_",
    "ref_src",
    "ref_url",
    "feature",
    "si",         // YouTube tracking
    "pp",         // YouTube tracking
    "ab_channel", // YouTube tracking
    // YouTube ephemeral context
    "t",           // timestamp (t=13s, t=1m30s)
    "list",        // playlist ID
    "index",       // playlist position
    "start_radio", // YouTube mix seed
    "flow",        // YouTube flow parameter
    "app",         // app source (app=desktop)
];

pub fn clean_url(raw: &str) -> eyre::Result<String> {
    let mut parsed = Url::parse(raw.trim())?;

    let cleaned_pairs: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !TRACKING_PARAMS.contains(&key.as_ref()))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    if cleaned_pairs.is_empty() {
        parsed.set_query(None);
    } else {
        let query = cleaned_pairs
            .iter()
            .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{k}={v}") })
            .collect::<Vec<_>>()
            .join("&");
        parsed.set_query(Some(&query));
    }

    // Remove trailing fragment if empty
    if parsed.fragment() == Some("") {
        parsed.set_fragment(None);
    }

    Ok(parsed.to_string())
}

/// Apply config-driven canonicalization rules to a cleaned URL.
/// First matching rule wins. If no rule matches, returns the URL unchanged.
pub fn canonicalize_url(url: &str, rules: &[CanonicalRule]) -> String {
    for rule in rules {
        let re = match regex::Regex::new(&rule.match_regex) {
            Ok(re) => re,
            Err(e) => {
                log::warn!("Invalid canonicalization regex for '{}': {e}", rule.name);
                continue;
            }
        };
        if let Some(caps) = re.captures(url) {
            let mut result = rule.canonical.clone();
            for name in re.capture_names().flatten() {
                if let Some(m) = caps.name(name) {
                    result = result.replace(&format!("{{{name}}}"), m.as_str());
                }
            }
            return result;
        }
    }
    url.to_string()
}

/// Combined: clean + canonicalize. This is what callers should use.
pub fn normalize_url(raw: &str, rules: &[CanonicalRule]) -> eyre::Result<String> {
    let cleaned = clean_url(raw)?;
    Ok(canonicalize_url(&cleaned, rules))
}

pub fn sanitize_tag(tag: &str) -> String {
    tag.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Valid domain values per system/domain-values.md in the vault.
const VALID_DOMAINS: &[&str] = &[
    "ai", "tech", "football", "work", "writing", "music", "spanish", "knowledge", "resources",
    "system",
];

/// Legacy folder-to-domain mapping for backward compat with old Fabric patterns
/// and any other code that might emit the old emoji folder paths.
const DOMAIN_ALIASES: &[(&str, &str)] = &[
    ("🤖 Tech/ai-llm", "ai"),
    ("🤖 tech/ai-llm", "ai"),
    ("tech/ai-llm", "ai"),
    ("ai-llm", "ai"),
    ("🤖 Tech/rust", "tech"),
    ("🤖 Tech/nixos", "tech"),
    ("🤖 Tech/python", "tech"),
    ("🤖 Tech/tools", "tech"),
    ("🤖 Tech/devops", "tech"),
    ("🤖 Tech/snippets", "tech"),
    ("🤖 tech", "tech"),
    ("🏈 Football/research", "football"),
    ("🏈 Football", "football"),
    ("✍️ Writing/craft", "writing"),
    ("✍️ Writing", "writing"),
    ("💼 Work", "work"),
    ("📚 Resources/articles", "resources"),
    ("📚 Resources/videos", "resources"),
    ("📚 Resources", "resources"),
    ("🧠 Knowledge/health", "knowledge"),
    ("🧠 Knowledge/learning", "knowledge"),
    ("🧠 Knowledge", "knowledge"),
    ("🎵 Music", "music"),
    ("🇪🇸 Spanish", "spanish"),
    ("⚙️ System", "system"),
    ("📥 Inbox", "inbox"),
    ("Inbox", "inbox"),
];

/// Normalize a domain value to the canonical v2 format.
///
/// Handles:
/// - Old emoji folder paths (e.g. "🤖 Tech/ai-llm" -> "ai")
/// - Case normalization (e.g. "AI" -> "ai")
/// - Already-valid values pass through
/// - Unknown values log a warning and pass through lowercased
pub fn normalize_domain(raw: &str) -> String {
    let trimmed = raw.trim();

    // Check exact alias match first (handles emoji paths)
    for &(alias, domain) in DOMAIN_ALIASES {
        if trimmed == alias {
            return domain.to_string();
        }
    }

    // Lowercase and check if it's already a valid domain
    let lower = trimmed.to_lowercase();
    if VALID_DOMAINS.contains(&lower.as_str()) {
        return lower;
    }

    // Try case-insensitive alias match
    let trimmed_lower = trimmed.to_lowercase();
    for &(alias, domain) in DOMAIN_ALIASES {
        if trimmed_lower == alias.to_lowercase() {
            return domain.to_string();
        }
    }

    log::warn!("Unknown domain value '{}', passing through as-is", trimmed);
    lower
}

pub fn sanitize_filename(title: &str) -> String {
    let sanitized: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect();

    // Collapse consecutive hyphens
    let mut result = String::new();
    let mut prev_hyphen = false;
    for c in sanitized.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }

    result.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_canonicalization_rules;

    #[test]
    fn test_clean_url_strips_youtube_ephemeral() {
        let url = "https://www.youtube.com/watch?v=abc&t=13s&list=PLxyz&index=3";
        let cleaned = clean_url(url).expect("valid url");
        assert_eq!(cleaned, "https://www.youtube.com/watch?v=abc");
    }

    #[test]
    fn test_clean_url_strips_start_radio_flow_app() {
        let url = "https://www.youtube.com/watch?v=abc&start_radio=1&flow=1&app=desktop";
        let cleaned = clean_url(url).expect("valid url");
        assert_eq!(cleaned, "https://www.youtube.com/watch?v=abc");
    }

    #[test]
    fn test_canonicalize_youtu_be() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://youtu.be/abc123", &rules);
        assert_eq!(result, "https://www.youtube.com/watch?v=abc123");
    }

    #[test]
    fn test_canonicalize_mobile_youtube() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://m.youtube.com/watch?v=abc123", &rules);
        assert_eq!(result, "https://www.youtube.com/watch?v=abc123");
    }

    #[test]
    fn test_canonicalize_music_youtube() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://music.youtube.com/watch?v=abc123", &rules);
        assert_eq!(result, "https://www.youtube.com/watch?v=abc123");
    }

    #[test]
    fn test_canonicalize_youtube_nocookie() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://www.youtube-nocookie.com/embed/abc123", &rules);
        assert_eq!(result, "https://www.youtube.com/watch?v=abc123");
    }

    #[test]
    fn test_canonicalize_mobile_shorts() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://m.youtube.com/shorts/abc123", &rules);
        assert_eq!(result, "https://www.youtube.com/shorts/abc123");
    }

    #[test]
    fn test_canonicalize_twitter_to_x() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://twitter.com/user/status/123", &rules);
        assert_eq!(result, "https://x.com/user/status/123");
    }

    #[test]
    fn test_canonicalize_mobile_twitter_to_x() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://mobile.twitter.com/user/status/123", &rules);
        assert_eq!(result, "https://x.com/user/status/123");
    }

    #[test]
    fn test_canonicalize_no_match_passthrough() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://example.com/page", &rules);
        assert_eq!(result, "https://example.com/page");
    }

    #[test]
    fn test_canonicalize_www_youtube_unchanged() {
        let rules = default_canonicalization_rules();
        let result = canonicalize_url("https://www.youtube.com/watch?v=abc123", &rules);
        // www.youtube.com doesn't match any canonicalization rule — passthrough
        assert_eq!(result, "https://www.youtube.com/watch?v=abc123");
    }

    #[test]
    fn test_normalize_url_full_pipeline() {
        let rules = default_canonicalization_rules();
        let result = normalize_url("https://youtu.be/abc123?si=tracking&t=45s", &rules).expect("valid");
        assert_eq!(result, "https://www.youtube.com/watch?v=abc123");
    }

    #[test]
    fn test_canonicalize_custom_rule() {
        let rules = vec![CanonicalRule {
            name: "old-reddit".to_string(),
            match_regex: r"https?://old\.reddit\.com/(?P<path>.*)".to_string(),
            canonical: "https://www.reddit.com/{path}".to_string(),
        }];
        let result = canonicalize_url("https://old.reddit.com/r/rust/top", &rules);
        assert_eq!(result, "https://www.reddit.com/r/rust/top");
    }

    #[test]
    fn test_clean_url_strips_utm() {
        let url = "https://example.com/page?utm_source=twitter&utm_medium=social&id=42";
        let cleaned = clean_url(url).expect("valid url");
        assert_eq!(cleaned, "https://example.com/page?id=42");
    }

    #[test]
    fn test_clean_url_strips_all_tracking() {
        let url = "https://example.com/page?utm_source=x&fbclid=abc&gclid=def";
        let cleaned = clean_url(url).expect("valid url");
        assert_eq!(cleaned, "https://example.com/page");
    }

    #[test]
    fn test_clean_url_preserves_non_tracking() {
        let url = "https://youtube.com/watch?v=abc123";
        let cleaned = clean_url(url).expect("valid url");
        assert_eq!(cleaned, "https://youtube.com/watch?v=abc123");
    }

    #[test]
    fn test_clean_url_strips_youtube_si() {
        let url = "https://www.youtube.com/watch?v=abc&si=tracking123&pp=stuff";
        let cleaned = clean_url(url).expect("valid url");
        assert_eq!(cleaned, "https://www.youtube.com/watch?v=abc");
    }

    #[test]
    fn test_clean_url_no_query() {
        let url = "https://example.com/page";
        let cleaned = clean_url(url).expect("valid url");
        assert_eq!(cleaned, "https://example.com/page");
    }

    #[test]
    fn test_clean_url_invalid() {
        let result = clean_url("not a url");
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_tag_basic() {
        assert_eq!(sanitize_tag("AI/ML"), "ai-ml");
    }

    #[test]
    fn test_sanitize_tag_spaces() {
        assert_eq!(sanitize_tag("Machine Learning"), "machine-learning");
    }

    #[test]
    fn test_sanitize_tag_special_chars() {
        assert_eq!(sanitize_tag("C++ Programming!"), "c---programming");
    }

    #[test]
    fn test_sanitize_tag_already_clean() {
        assert_eq!(sanitize_tag("rust"), "rust");
    }

    #[test]
    fn test_sanitize_tag_trim_hyphens() {
        assert_eq!(sanitize_tag("--hello--"), "hello");
    }

    #[test]
    fn test_sanitize_filename_basic() {
        assert_eq!(sanitize_filename("Hello World!"), "hello-world");
    }

    #[test]
    fn test_sanitize_filename_special() {
        assert_eq!(sanitize_filename("Test: A/B \"quotes\""), "test-a-b-quotes");
    }

    #[test]
    fn test_sanitize_filename_normal() {
        assert_eq!(sanitize_filename("normal-file_name"), "normal-file-name");
    }

    #[test]
    fn test_sanitize_filename_collapses_spaces() {
        assert_eq!(sanitize_filename("a:::b"), "a-b");
    }

    #[test]
    fn test_normalize_domain_valid_passthrough() {
        assert_eq!(normalize_domain("ai"), "ai");
        assert_eq!(normalize_domain("tech"), "tech");
        assert_eq!(normalize_domain("football"), "football");
        assert_eq!(normalize_domain("resources"), "resources");
    }

    #[test]
    fn test_normalize_domain_emoji_folders() {
        assert_eq!(normalize_domain("🤖 Tech/ai-llm"), "ai");
        assert_eq!(normalize_domain("🤖 Tech/rust"), "tech");
        assert_eq!(normalize_domain("🤖 Tech/tools"), "tech");
        assert_eq!(normalize_domain("🏈 Football/research"), "football");
        assert_eq!(normalize_domain("✍️ Writing/craft"), "writing");
        assert_eq!(normalize_domain("💼 Work"), "work");
        assert_eq!(normalize_domain("📚 Resources/articles"), "resources");
        assert_eq!(normalize_domain("🧠 Knowledge/health"), "knowledge");
        assert_eq!(normalize_domain("🎵 Music"), "music");
        assert_eq!(normalize_domain("🇪🇸 Spanish"), "spanish");
        assert_eq!(normalize_domain("⚙️ System"), "system");
    }

    #[test]
    fn test_normalize_domain_case_insensitive() {
        assert_eq!(normalize_domain("AI"), "ai");
        assert_eq!(normalize_domain("Tech"), "tech");
        assert_eq!(normalize_domain("FOOTBALL"), "football");
    }

    #[test]
    fn test_normalize_domain_trimming() {
        assert_eq!(normalize_domain("  ai  "), "ai");
        assert_eq!(normalize_domain(" 🤖 Tech/ai-llm "), "ai");
    }
}
