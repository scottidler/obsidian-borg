#![deny(clippy::unwrap_used)]
#![deny(dead_code)]
#![deny(unused_variables)]

use clap::Parser;
use colored::*;
use eyre::{Context, Result};
use log::info;

mod cli;
mod config;

use cli::Cli;
use config::Config;

fn run_application(cli: &Cli, config: &Config) -> Result<()> {
    info!("Starting borg-daemon");

    println!("{}", "Configuration loaded successfully".green());
    if cli.verbose {
        println!("{}", "Verbose mode enabled".yellow());
    }
    if config.debug {
        println!("{}", "Debug mode enabled".yellow());
    }

    println!("borg-daemon v{}", env!("GIT_DESCRIBE"));
    println!("Server: {}:{}", config.server.host, config.server.port);

    info!("borg-daemon started successfully");

    Ok(())
}

fn main() -> Result<()> {
    borg_core::setup_logging("borg-daemon").context("Failed to setup logging")?;

    let cli = Cli::parse();

    let config: Config =
        borg_core::load_config("borg-daemon", cli.config.as_ref()).context("Failed to load configuration")?;

    info!("Starting with config from: {:?}", cli.config);

    run_application(&cli, &config).context("Application failed")?;

    Ok(())
}
