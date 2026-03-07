use clap::Parser;
use eyre::{Context, Result};
use obsidian_borg::cli::{Cli, Command};
use obsidian_borg::config::Config;
use obsidian_borg::logging;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config: Config =
        obsidian_borg::config::load_config(cli.config.as_ref()).context("Failed to load configuration")?;

    let log_level = logging::resolve_log_level(cli.log_level.as_deref(), config.log_level.as_deref());
    logging::setup_logging(&log_level).context("Failed to setup logging")?;

    log::info!("Starting obsidian-borg with config from: {:?}", cli.config);
    log::debug!("Resolved log level: {log_level}");
    log::debug!("Config: {:?}", config);

    if cli.verbose {
        println!("{}", colored::Colorize::yellow("Verbose mode enabled"));
    }

    match cli.command {
        None => {
            Cli::parse_from(["obsidian-borg", "--help"]);
            Ok(())
        }
        Some(Command::Daemon(opts)) => obsidian_borg::run_daemon(config, cli.verbose, opts).await,
        Some(Command::Ingest { url, tags }) => obsidian_borg::run_ingest(config, url, tags).await,
    }
}
