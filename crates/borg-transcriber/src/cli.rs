use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "borg-transcriber",
    about = "Whisper transcription microservice for obsidian-borg",
    version = env!("GIT_DESCRIBE"),
    after_help = "Logs are written to: ~/.local/share/borg-transcriber/logs/borg-transcriber.log"
)]
pub struct Cli {
    /// Path to config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,
}
