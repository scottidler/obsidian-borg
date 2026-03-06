use crate::config::{Config, TelegramConfig};
use eyre::Result;
use std::sync::Arc;

pub async fn run(_token: String, _tg_config: TelegramConfig, _config: Arc<Config>) -> Result<()> {
    // Implementation in Phase 3
    Ok(())
}
