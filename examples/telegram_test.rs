use teloxide::prelude::*;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("trace"))
        .init();

    let token = std::env::var("TELEGRAM_BOT_TOKEN").expect("TELEGRAM_BOT_TOKEN must be set");
    let bot = Bot::new(token);

    println!("Starting minimal teloxide dispatch test...");

    let handler = Update::filter_message().endpoint(|_msg: Message, _bot: Bot| async move {
        println!("got a message");
        Ok::<(), teloxide::RequestError>(())
    });

    Dispatcher::builder(bot, handler)
        .build()
        .dispatch()
        .await;
}
