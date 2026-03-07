use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "obsidian-borg",
    about = "Obsidian ingestion daemon - receives URLs and produces summarized markdown notes",
    version = env!("GIT_DESCRIBE"),
    after_help = "Logs are written to: ~/.local/share/obsidian-borg/logs/obsidian-borg.log"
)]
pub struct Cli {
    /// Path to config file
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Log level (trace, debug, info, warn, error)
    #[arg(short, long)]
    pub log_level: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the ingestion daemon (default)
    Serve,
    /// Send a URL to the running daemon for ingestion
    Ingest {
        /// URL to ingest
        url: String,
        /// Comma-separated tags
        #[arg(short, long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    /// Install as a system service (systemd on Linux, launchd on macOS)
    Install {
        /// Overwrite existing service file
        #[arg(long)]
        force: bool,
    },
    /// Remove the system service
    Uninstall,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn test_no_subcommand_defaults_to_none() {
        let cli = Cli::try_parse_from(["obsidian-borg"]).expect("parse");
        assert!(cli.command.is_none());
    }

    #[test]
    fn test_serve_subcommand() {
        let cli = Cli::try_parse_from(["obsidian-borg", "serve"]).expect("parse");
        assert!(matches!(cli.command, Some(Command::Serve)));
    }

    #[test]
    fn test_ingest_subcommand() {
        let cli = Cli::try_parse_from(["obsidian-borg", "ingest", "https://example.com"]).expect("parse");
        match cli.command {
            Some(Command::Ingest { url, tags }) => {
                assert_eq!(url, "https://example.com");
                assert!(tags.is_none());
            }
            _ => panic!("expected Ingest"),
        }
    }

    #[test]
    fn test_ingest_with_tags() {
        let cli =
            Cli::try_parse_from(["obsidian-borg", "ingest", "https://example.com", "-t", "ai,rust"]).expect("parse");
        match cli.command {
            Some(Command::Ingest { url, tags }) => {
                assert_eq!(url, "https://example.com");
                assert_eq!(tags, Some(vec!["ai".to_string(), "rust".to_string()]));
            }
            _ => panic!("expected Ingest"),
        }
    }

    #[test]
    fn test_install_subcommand() {
        let cli = Cli::try_parse_from(["obsidian-borg", "install"]).expect("parse");
        match cli.command {
            Some(Command::Install { force }) => assert!(!force),
            _ => panic!("expected Install"),
        }
    }

    #[test]
    fn test_install_with_force() {
        let cli = Cli::try_parse_from(["obsidian-borg", "install", "--force"]).expect("parse");
        match cli.command {
            Some(Command::Install { force }) => assert!(force),
            _ => panic!("expected Install"),
        }
    }

    #[test]
    fn test_uninstall_subcommand() {
        let cli = Cli::try_parse_from(["obsidian-borg", "uninstall"]).expect("parse");
        assert!(matches!(cli.command, Some(Command::Uninstall)));
    }

    #[test]
    fn test_global_options_with_subcommand() {
        let cli = Cli::try_parse_from(["obsidian-borg", "-v", "-l", "debug", "serve"]).expect("parse");
        assert!(cli.verbose);
        assert_eq!(cli.log_level, Some("debug".to_string()));
        assert!(matches!(cli.command, Some(Command::Serve)));
    }
}
