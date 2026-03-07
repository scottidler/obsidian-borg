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

pub fn sanitize_tag(tag: &str) -> String {
    tag.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

pub fn sanitize_filename(title: &str) -> String {
    let sanitized: String = title
        .chars()
        .map(
            |c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { ' ' }
            },
        )
        .collect();

    // Collapse multiple spaces
    let mut result = String::new();
    let mut prev_space = false;
    for c in sanitized.chars() {
        if c == ' ' {
            if !prev_space {
                result.push(c);
            }
            prev_space = true;
        } else {
            result.push(c);
            prev_space = false;
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(sanitize_filename("Hello World!"), "Hello World");
    }

    #[test]
    fn test_sanitize_filename_special() {
        assert_eq!(sanitize_filename("Test: A/B \"quotes\""), "Test A B quotes");
    }

    #[test]
    fn test_sanitize_filename_normal() {
        assert_eq!(sanitize_filename("normal-file_name"), "normal-file_name");
    }

    #[test]
    fn test_sanitize_filename_collapses_spaces() {
        assert_eq!(sanitize_filename("a:::b"), "a b");
    }
}
