use url::Url;

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
}
