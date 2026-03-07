use eyre::{Context, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const APP_NAME: &str = "obsidian-borg";

/// Load configuration with fallback chain:
/// 1. Explicit path (if provided)
/// 2. ~/.config/obsidian-borg/obsidian-borg.yml
/// 3. ./obsidian-borg.yml
/// 4. Default
pub fn load_config<T: DeserializeOwned + Default>(config_path: Option<&PathBuf>) -> Result<T> {
    if let Some(path) = config_path {
        return load_from_file(path).context(format!("Failed to load config from {}", path.display()));
    }

    if let Some(config_dir) = dirs::config_dir() {
        let primary_config = config_dir.join(APP_NAME).join(format!("{APP_NAME}.yml"));
        if primary_config.exists() {
            match load_from_file(&primary_config) {
                Ok(config) => return Ok(config),
                Err(e) => {
                    log::warn!("Failed to load config from {}: {}", primary_config.display(), e);
                }
            }
        }
    }

    let fallback_config = PathBuf::from(format!("{APP_NAME}.yml"));
    if fallback_config.exists() {
        match load_from_file(&fallback_config) {
            Ok(config) => return Ok(config),
            Err(e) => {
                log::warn!("Failed to load config from {}: {}", fallback_config.display(), e);
            }
        }
    }

    log::info!("No config file found, using defaults");
    Ok(T::default())
}

fn load_from_file<T: DeserializeOwned, P: AsRef<Path>>(path: P) -> Result<T> {
    let content = fs::read_to_string(&path).context("Failed to read config file")?;
    let config: T = serde_yaml::from_str(&content).context("Failed to parse config file")?;
    log::info!("Loaded config from: {}", path.as_ref().display());
    Ok(config)
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub vault: VaultConfig,
    pub transcriber: TranscriberConfig,
    pub groq: GroqConfig,
    pub llm: LlmConfig,
    pub telegram: Option<TelegramConfig>,
    pub discord: Option<DiscordConfig>,
    #[serde(default = "default_links")]
    pub links: Vec<LinkConfig>,
    pub fabric: FabricConfig,
    pub frontmatter: FrontmatterConfig,
    pub log_level: Option<String>,
    pub debug: bool,
}

fn default_links() -> Vec<LinkConfig> {
    vec![
        LinkConfig {
            name: "shorts".to_string(),
            regex: r"https?://(?:www\.)?youtube\.com/shorts/([a-zA-Z0-9_-]+)".to_string(),
            resolution: "480p".to_string(),
            folder: "".to_string(),
        },
        LinkConfig {
            name: "youtube".to_string(),
            regex:
                r"https?://(?:www\.)?(youtube\.com/watch\?v=|youtu\.be/|music\.youtube\.com/watch\?v=)([a-zA-Z0-9_-]+)"
                    .to_string(),
            resolution: "FWVGA".to_string(),
            folder: "".to_string(),
        },
        LinkConfig {
            name: "default".to_string(),
            regex: r".*".to_string(),
            resolution: "FWVGA".to_string(),
            folder: "".to_string(),
        },
    ]
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LinkConfig {
    pub name: String,
    pub regex: String,
    #[serde(default = "default_resolution")]
    pub resolution: String,
    #[serde(default)]
    pub folder: String,
}

fn default_resolution() -> String {
    "FWVGA".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FabricConfig {
    pub binary: String,
    pub model: String,
    pub summarize_pattern_youtube: String,
    pub summarize_pattern_article: String,
    pub tag_pattern: String,
    pub classify_pattern: String,
    pub max_content_chars: usize,
}

impl Default for FabricConfig {
    fn default() -> Self {
        Self {
            binary: "fabric".to_string(),
            model: String::new(),
            summarize_pattern_youtube: "youtube_summary".to_string(),
            summarize_pattern_article: "extract_article_wisdom".to_string(),
            tag_pattern: "create_tags".to_string(),
            classify_pattern: "obsidian_classify".to_string(),
            max_content_chars: 30000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FrontmatterConfig {
    #[serde(default)]
    pub default_tags: Vec<String>,
    #[serde(default)]
    pub default_author: String,
    pub timezone: String,
}

impl Default for FrontmatterConfig {
    fn default() -> Self {
        Self {
            default_tags: vec![],
            default_author: String::new(),
            timezone: "America/Los_Angeles".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TelegramConfig {
    pub bot_token_env: String,
    #[serde(default)]
    pub allowed_chat_ids: Vec<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DiscordConfig {
    pub bot_token_env: String,
    pub channel_id: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct VaultConfig {
    pub inbox_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct TranscriberConfig {
    pub url: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct GroqConfig {
    pub api_key_env: String,
    pub model: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct LlmConfig {
    pub provider: String,
    pub model: String,
    pub api_key_env: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
        }
    }
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            inbox_path: "~/obsidian-vault/Inbox".to_string(),
        }
    }
}

impl Default for TranscriberConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:8090".to_string(),
            timeout_secs: 120,
        }
    }
}

impl Default for GroqConfig {
    fn default() -> Self {
        Self {
            api_key_env: "GROQ_API_KEY".to_string(),
            model: "whisper-large-v3".to_string(),
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "claude".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, Default, PartialEq)]
    struct TestConfig {
        #[serde(default)]
        name: String,
    }

    #[test]
    fn test_load_config_returns_default_when_no_file() {
        let config: TestConfig = load_config(None).expect("should succeed");
        assert_eq!(config, TestConfig::default());
    }

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.transcriber.url, "http://localhost:8090");
        assert_eq!(config.groq.model, "whisper-large-v3");
        assert_eq!(config.llm.provider, "claude");
        assert!(!config.debug);
    }

    #[test]
    fn test_config_deserialize() {
        let yaml = r#"
server:
  host: "127.0.0.1"
  port: 9090
vault:
  inbox_path: "/tmp/vault/Inbox"
transcriber:
  url: "http://192.168.1.100:8090"
  timeout_secs: 60
groq:
  model: "whisper-large-v3-turbo"
llm:
  provider: "ollama"
  model: "llama3"
debug: true
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.vault.inbox_path, "/tmp/vault/Inbox");
        assert_eq!(config.transcriber.url, "http://192.168.1.100:8090");
        assert_eq!(config.transcriber.timeout_secs, 60);
        assert_eq!(config.groq.model, "whisper-large-v3-turbo");
        assert_eq!(config.llm.provider, "ollama");
        assert!(config.debug);
    }

    #[test]
    fn test_config_without_bot_sections() {
        let yaml = r#"
server:
  host: "0.0.0.0"
  port: 8080
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        assert!(config.telegram.is_none());
        assert!(config.discord.is_none());
    }

    #[test]
    fn test_config_with_telegram_section() {
        let yaml = r#"
telegram:
  bot_token_env: "TELEGRAM_BOT_TOKEN"
  allowed_chat_ids: [123456, 789012]
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        let tg = config.telegram.expect("telegram should be Some");
        assert_eq!(tg.bot_token_env, "TELEGRAM_BOT_TOKEN");
        assert_eq!(tg.allowed_chat_ids, vec![123456, 789012]);
    }

    #[test]
    fn test_config_with_telegram_no_allowed_ids() {
        let yaml = r#"
telegram:
  bot_token_env: "TELEGRAM_BOT_TOKEN"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        let tg = config.telegram.expect("telegram should be Some");
        assert!(tg.allowed_chat_ids.is_empty());
    }

    #[test]
    fn test_config_with_discord_section() {
        let yaml = r#"
discord:
  bot_token_env: "DISCORD_BOT_TOKEN"
  channel_id: 1234567890
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        let dc = config.discord.expect("discord should be Some");
        assert_eq!(dc.bot_token_env, "DISCORD_BOT_TOKEN");
        assert_eq!(dc.channel_id, 1234567890);
    }

    #[test]
    fn test_config_with_both_bots() {
        let yaml = r#"
telegram:
  bot_token_env: "TG_TOKEN"
discord:
  bot_token_env: "DC_TOKEN"
  channel_id: 999
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        assert!(config.telegram.is_some());
        assert!(config.discord.is_some());
    }
}
