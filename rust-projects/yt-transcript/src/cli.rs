use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Extract YouTube video transcripts to local markdown files")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// YouTube video URL (when not using a subcommand)
    #[arg(value_name = "URL", conflicts_with = "command")]
    pub url: Option<String>,

    /// Print transcript to stdout instead of saving to file
    #[arg(long)]
    pub stdout: bool,

    /// Preferred subtitle language (default: en)
    #[arg(long, default_value = "en")]
    pub lang: String,

    /// Output directory (default: ~/Media/transcripts)
    #[arg(long, short)]
    pub output_dir: Option<std::path::PathBuf>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Fetch transcripts for videos from a YouTube channel
    Channel {
        /// YouTube channel URL
        url: String,

        /// Maximum number of videos to process
        #[arg(long, default_value = "10")]
        limit: usize,

        /// Preferred subtitle language
        #[arg(long, default_value = "en")]
        lang: String,

        /// Output directory
        #[arg(long, short)]
        output_dir: Option<std::path::PathBuf>,
    },
}
