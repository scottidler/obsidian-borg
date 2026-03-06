use chrono::Utc;

pub struct NoteContent {
    pub title: String,
    pub source_url: String,
    pub tags: Vec<String>,
    pub summary: String,
    pub content_type: ContentType,
}

pub enum ContentType {
    YouTube { uploader: String, duration_secs: f64 },
    Article,
}

pub fn render_note(note: &NoteContent) -> String {
    let date = Utc::now().format("%Y-%m-%d").to_string();
    let tags_yaml = note
        .tags
        .iter()
        .map(|t| format!("  - {t}"))
        .collect::<Vec<_>>()
        .join("\n");

    let type_field = match &note.content_type {
        ContentType::YouTube { .. } => "youtube",
        ContentType::Article => "article",
    };

    let mut frontmatter = format!(
        "---\ntitle: \"{}\"\ndate: {date}\nsource: \"{}\"\ntype: {type_field}\ntags:\n{tags_yaml}\n",
        escape_yaml_string(&note.title),
        note.source_url,
    );

    if let ContentType::YouTube {
        uploader,
        duration_secs,
    } = &note.content_type
    {
        let minutes = (*duration_secs / 60.0).round() as u32;
        frontmatter.push_str(&format!("uploader: \"{uploader}\"\nduration_min: {minutes}\n"));
    }

    frontmatter.push_str("---\n\n");

    let body = format!(
        "# {}\n\n{}\n\n---\n\n*Source: [{}]({})*\n",
        note.title, note.summary, note.source_url, note.source_url
    );

    format!("{frontmatter}{body}")
}

fn escape_yaml_string(s: &str) -> String {
    s.replace('"', "\\\"")
}

pub fn sanitize_filename(title: &str) -> String {
    title
        .chars()
        .map(
            |c| {
                if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }
            },
        )
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_article_note() {
        let note = NoteContent {
            title: "Test Article".to_string(),
            source_url: "https://example.com/post".to_string(),
            tags: vec!["rust".to_string(), "programming".to_string()],
            summary: "This is a summary.".to_string(),
            content_type: ContentType::Article,
        };
        let rendered = render_note(&note);
        assert!(rendered.contains("title: \"Test Article\""));
        assert!(rendered.contains("type: article"));
        assert!(rendered.contains("  - rust"));
        assert!(rendered.contains("This is a summary."));
        assert!(rendered.contains("Source: [https://example.com/post]"));
    }

    #[test]
    fn test_render_youtube_note() {
        let note = NoteContent {
            title: "Cool Video".to_string(),
            source_url: "https://youtube.com/watch?v=abc".to_string(),
            tags: vec!["youtube".to_string()],
            summary: "Video summary here.".to_string(),
            content_type: ContentType::YouTube {
                uploader: "TechChannel".to_string(),
                duration_secs: 600.0,
            },
        };
        let rendered = render_note(&note);
        assert!(rendered.contains("type: youtube"));
        assert!(rendered.contains("uploader: \"TechChannel\""));
        assert!(rendered.contains("duration_min: 10"));
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("Hello World!"), "Hello World_");
        assert_eq!(sanitize_filename("Test: A/B \"quotes\""), "Test_ A_B _quotes_");
        assert_eq!(sanitize_filename("normal-file_name"), "normal-file_name");
    }

    #[test]
    fn test_escape_yaml_string() {
        assert_eq!(escape_yaml_string("He said \"hello\""), "He said \\\"hello\\\"");
    }
}
