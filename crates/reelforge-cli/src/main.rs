//! `ReelForge` command-line entry point.

mod commands;

use clap::{Parser, Subcommand};
use reelforge::VERSION;

#[derive(Parser)]
#[command(name = "reelforge", version = VERSION, about = "Programmatic video editing for Rust")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print version information.
    Version,
    /// Probe media metadata via ffprobe.
    Probe {
        /// Input media path.
        path: String,
    },
    /// Cut a subclip and write it (`FFmpeg` filtergraph fast path).
    Cut {
        /// Input media path.
        input: String,
        /// Output media path.
        output: String,
        /// Start time in seconds.
        #[arg(long)]
        start: f64,
        /// Duration in seconds.
        #[arg(long)]
        duration: f64,
    },
    /// Apply a simple filtergraph and write (hflip/vflip/scale).
    Filter {
        /// Input media path.
        input: String,
        /// Output media path.
        output: String,
        /// Horizontal flip.
        #[arg(long)]
        hflip: bool,
        /// Vertical flip.
        #[arg(long)]
        vflip: bool,
        /// Scale width (optional).
        #[arg(long)]
        width: Option<u32>,
        /// Scale height (optional).
        #[arg(long)]
        height: Option<u32>,
    },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Version => {
            commands::version::run();
            Ok(())
        }
        Commands::Probe { path } => commands::probe::run(&path),
        Commands::Cut {
            input,
            output,
            start,
            duration,
        } => commands::cut::run(&input, &output, start, duration),
        Commands::Filter {
            input,
            output,
            hflip,
            vflip,
            width,
            height,
        } => commands::filter::run(&input, &output, hflip, vflip, width, height),
    }
}
