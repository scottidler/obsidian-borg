use crate::backoff::ExponentialBackoff;
use crate::config::{Config, DiscordConfig};
use crate::pipeline;
use crate::router::{extract_url_from_text, format_reply};
use crate::types::{ContentKind, IngestMethod};
use eyre::Result;
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::model::gateway::GatewayIntents;
use serenity::prelude::*;
use std::sync::Arc;

struct Handler {
    config: Arc<Config>,
    channel_id: u64,
}

#[async_trait]
impl EventHandler for Handler {
    async fn message(&self, ctx: serenity::prelude::Context, msg: Message) {
        if msg.channel_id.get() != self.channel_id || msg.author.bot {
            return;
        }

        let (content, display_source) = if let Some(url) = extract_url_from_text(&msg.content) {
            (ContentKind::Url(url.clone()), url)
        } else if !msg.content.trim().is_empty() {
            let display = if msg.content.len() > 50 {
                format!("{}...", &msg.content[..50])
            } else {
                msg.content.clone()
            };
            (ContentKind::Text(msg.content.clone()), display)
        } else {
            return;
        };

        let _ = msg.channel_id.say(&ctx.http, "Processing...").await;
        let result = pipeline::process_content(content, vec![], IngestMethod::Discord, false, &self.config).await;
        let _ = msg
            .channel_id
            .say(&ctx.http, format_reply(&result, &display_source))
            .await;
    }
}

pub async fn run(token: String, dc_config: DiscordConfig, config: Arc<Config>) -> Result<()> {
    let mut backoff = ExponentialBackoff::new();

    loop {
        log::info!("discord: starting bot");
        let handler = Handler {
            config: config.clone(),
            channel_id: dc_config.channel_id,
        };
        let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

        let client = match serenity::Client::builder(&token, intents).event_handler(handler).await {
            Ok(c) => c,
            Err(e) => {
                log::error!("discord: failed to create client: {e}");
                backoff.wait().await;
                continue;
            }
        };

        backoff.reset();

        let mut client = client;
        if let Err(e) = client.start().await {
            log::error!("discord: client error: {e}");
        } else {
            log::warn!("discord: client exited, will restart");
        }

        backoff.wait().await;
    }
}
