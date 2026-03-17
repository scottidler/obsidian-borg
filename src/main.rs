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
        Some(Command::Ingest {
            url,
            clipboard,
            file,
            tags,
            force,
        }) => {
            if let Some(file_path) = file {
                obsidian_borg::run_file_ingest(config, file_path, tags, force).await
            } else {
                let resolved_url = obsidian_borg::resolve_ingest_url(url, clipboard)?;
                let method = if clipboard {
                    obsidian_borg::types::IngestMethod::Clipboard
                } else {
                    obsidian_borg::types::IngestMethod::Cli
                };
                obsidian_borg::run_ingest(config, resolved_url, tags, force, clipboard, method).await
            }
        }
        Some(Command::Note { text, clipboard, tags }) => {
            let resolved_text = obsidian_borg::resolve_note_text(text, clipboard)?;
            obsidian_borg::run_note(config, resolved_text, tags).await
        }
        Some(Command::Hotkey(opts)) => obsidian_borg::run_hotkey(opts, &config).await,
        Some(Command::Sign) => obsidian_borg::run_sign(&config).await,
        Some(Command::Migrate { dry_run: _, apply }) => obsidian_borg::migrate::run_migrate(&config, apply).await,
        Some(Command::Audit { fix }) => obsidian_borg::audit::run_audit(&config, fix).await,
    }
}
