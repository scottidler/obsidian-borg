use crate::config::{Config, TelegramConfig};
use crate::pipeline;
use crate::router::{extract_url_from_text, format_reply};
use eyre::Result;
use std::sync::Arc;
use teloxide::prelude::*;

pub async fn run(token: String, tg_config: TelegramConfig, config: Arc<Config>) -> Result<()> {
    let bot = teloxide::Bot::new(&token);

    let handler = Update::filter_message().endpoint(move |message: Message, bot: Bot| {
        let config = config.clone();
        let allowed = tg_config.allowed_chat_ids.clone();
        async move {
            if !allowed.is_empty() && !allowed.contains(&message.chat.id.0) {
                return Ok::<(), teloxide::RequestError>(());
            }

            let text = message.text().unwrap_or("");
            log::debug!("Telegram message from chat {}: {text}", message.chat.id);
            let Some(url) = extract_url_from_text(text) else {
                log::debug!("No URL found in message");
                bot.send_message(message.chat.id, "No URL found in message.").await?;
                return Ok(());
            };

            log::info!("Telegram: processing URL {url} from chat {}", message.chat.id);
            bot.send_message(message.chat.id, "Processing...").await?;

            let result = pipeline::process_url(&url, vec![], &config).await;
            log::debug!("Pipeline result: {:?}", result.status);
            let reply = format_reply(&result, &url);
            bot.send_message(message.chat.id, reply).await?;

            Ok(())
        }
    });

    Dispatcher::builder(bot, handler)
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
