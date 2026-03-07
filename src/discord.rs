use crate::config::{Config, DiscordConfig};
use crate::pipeline;
use crate::router::{extract_url_from_text, format_reply};
use crate::types::IngestMethod;
use eyre::{Context, Result};
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

        let Some(url) = extract_url_from_text(&msg.content) else {
            return;
        };

        let _ = msg.channel_id.say(&ctx.http, "Processing...").await;
        let result = pipeline::process_url(&url, vec![], IngestMethod::Discord, false, &self.config).await;
        let _ = msg.channel_id.say(&ctx.http, format_reply(&result, &url)).await;
    }
}

pub async fn run(token: String, dc_config: DiscordConfig, config: Arc<Config>) -> Result<()> {
    let handler = Handler {
        config,
        channel_id: dc_config.channel_id,
    };
    let intents = GatewayIntents::GUILD_MESSAGES | GatewayIntents::MESSAGE_CONTENT;

    let mut client = serenity::Client::builder(&token, intents)
        .event_handler(handler)
        .await
        .context("Failed to create Discord client")?;

    client.start().await.context("Discord client error")?;
    Ok(())
}
