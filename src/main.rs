use clap::Parser;
use eyre::{Context, Result};

#[tokio::main]
async fn main() -> Result<()> {
    obsidian_borg::logging::setup_logging().context("Failed to setup logging")?;
    let cli = obsidian_borg::cli::Cli::parse();
    let config = obsidian_borg::config::load_config(cli.config.as_ref()).context("Failed to load configuration")?;

    log::info!("Starting obsidian-borg with config from: {:?}", cli.config);

    if cli.verbose {
        println!("{}", colored::Colorize::yellow("Verbose mode enabled"));
    }

    obsidian_borg::run_server(config, cli.verbose).await
}
