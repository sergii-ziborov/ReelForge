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
    /// Inspect / optimize / extract / run a JSON `RenderPlan`.
    Plan {
        /// Path to `RenderPlan` JSON.
        path: String,
        /// Print extraction summary (default).
        #[arg(long, group = "mode")]
        explain: bool,
        /// Emit optimized plan JSON.
        #[arg(long, group = "mode")]
        optimize: bool,
        /// Emit extraction JSON (`FFmpeg` prefix + remainder).
        #[arg(long, group = "mode")]
        extract: bool,
        /// Execute fully `FFmpeg`-extractable plan (`output` required in JSON).
        #[arg(long, group = "mode")]
        run: bool,
        /// Optional output path for `--optimize` / `--extract` JSON.
        #[arg(long)]
        out: Option<String>,
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
        Commands::Plan {
            path,
            explain,
            optimize,
            extract,
            run,
            out,
        } => {
            use commands::plan::PlanMode;
            let mode = if run {
                PlanMode::Run
            } else if optimize {
                PlanMode::Optimize
            } else if extract {
                PlanMode::Extract
            } else {
                // default + explicit --explain
                let _ = explain;
                PlanMode::Explain
            };
            commands::plan::run(&path, mode, out.as_deref())
        }
    }
}
