use eyre::{Context, Result};
use std::fs;
use std::path::PathBuf;

const APP_NAME: &str = "obsidian-borg";

pub fn setup_logging() -> Result<()> {
    let log_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_NAME)
        .join("logs");

    fs::create_dir_all(&log_dir).context("Failed to create log directory")?;

    let log_file = log_dir.join(format!("{APP_NAME}.log"));

    let target = Box::new(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .context("Failed to open log file")?,
    );

    env_logger::Builder::from_default_env()
        .target(env_logger::Target::Pipe(target))
        .init();

    log::info!("Logging initialized, writing to: {}", log_file.display());
    Ok(())
}
