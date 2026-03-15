use crate::backoff::ExponentialBackoff;
use crate::config::{Config, TelegramConfig};
use crate::pipeline;
use crate::router::{extract_url_from_text, format_reply};
use crate::types::IngestMethod;
use eyre::Result;
use std::sync::Arc;
use teloxide::prelude::*;

pub async fn run(token: String, tg_config: TelegramConfig, config: Arc<Config>) -> Result<()> {
    let mut backoff = ExponentialBackoff::new();

    loop {
        log::info!("telegram: starting bot dispatcher");
        let bot = teloxide::Bot::new(&token);

        // Pre-flight check: verify we can reach the Telegram API
        match bot.get_me().await {
            Ok(me) => {
                log::info!("telegram: connected as @{}", me.username());
                backoff.reset();
            }
            Err(e) => {
                log::error!("telegram: cannot reach API: {e}");
                backoff.wait().await;
                continue;
            }
        }

        let tg = tg_config.clone();
        let cfg = config.clone();

        let handler = Update::filter_message().endpoint(move |message: Message, bot: Bot| {
            let config = cfg.clone();
            let allowed = tg.allowed_chat_ids.clone();
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

                let chat_id = message.chat.id;
                let bot_clone = bot.clone();
                tokio::spawn(async move {
                    let result = pipeline::process_url(&url, vec![], IngestMethod::Telegram, false, &config).await;
                    log::debug!("Pipeline result: {:?}", result.status);
                    let reply = format_reply(&result, &url);
                    if let Err(e) = bot_clone.send_message(chat_id, reply).await {
                        log::error!("Failed to send Telegram reply: {e}");
                    }
                });

                Ok(())
            }
        });

        // Catch panics from dispatch (teloxide panics on network errors during init)
        let result = std::panic::AssertUnwindSafe(async {
            Dispatcher::builder(bot, handler)
                .enable_ctrlc_handler()
                .build()
                .dispatch()
                .await;
        });

        match tokio::task::spawn(result).await {
            Ok(()) => {
                log::warn!("telegram: dispatcher exited, will restart");
            }
            Err(e) => {
                log::error!("telegram: dispatcher panicked: {e}");
            }
        }

        backoff.wait().await;
    }
}
