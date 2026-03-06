use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "borg-daemon",
    about = "Obsidian ingestion daemon - receives URLs and produces summarized markdown notes",
    version = env!("GIT_DESCRIBE"),
    after_help = "Logs are written to: ~/.local/share/borg-daemon/logs/borg-daemon.log"
)]
pub struct Cli {
    /// Path to config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,
}
