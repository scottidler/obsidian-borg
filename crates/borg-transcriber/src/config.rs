use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub whisper: WhisperConfig,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct WhisperConfig {
    pub model: String,
    pub model_path: String,
    pub device: String,
    pub compute_type: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8090,
        }
    }
}

impl Default for WhisperConfig {
    fn default() -> Self {
        Self {
            model: "large-v3".to_string(),
            model_path: "~/.local/share/whisper/models".to_string(),
            device: "cuda".to_string(),
            compute_type: "float16".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8090);
        assert_eq!(config.whisper.model, "large-v3");
        assert_eq!(config.whisper.device, "cuda");
    }

    #[test]
    fn test_config_deserialize() {
        let yaml = r#"
server:
  host: "127.0.0.1"
  port: 9999
whisper:
  model: "base"
  device: "cpu"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("should parse");
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9999);
        assert_eq!(config.whisper.model, "base");
        assert_eq!(config.whisper.device, "cpu");
    }
}
