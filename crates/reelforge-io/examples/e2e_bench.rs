//! End-to-end bench harness (privacy / edit / A/V, codecs, p50/p95, RSS).
//!
//! ```bash
//! cargo run -p reelforge-io --example e2e_bench --release -- --quick
//! cargo run -p reelforge-io --example e2e_bench --release -- --input clip.mp4 --full --json
//! ```
#![allow(clippy::print_stdout, clippy::print_stderr)]

use reelforge_io::{format_report, full_cases, run_e2e_case, smoke_cases, standard_cases};
use std::env;
use std::path::{Path, PathBuf};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let quick = args.iter().any(|a| a == "--quick");
    let full = args.iter().any(|a| a == "--full");
    let json = args.iter().any(|a| a == "--json");
    let input = flag_value(&args, "--input").map(PathBuf::from);
    let out_dir = flag_value(&args, "--out-dir")
        .map_or_else(|| PathBuf::from("target/demo/e2e"), PathBuf::from);
    let repeats: u32 = flag_value(&args, "--repeats")
        .and_then(|s| s.parse().ok())
        .unwrap_or(if quick { 2 } else { 3 });

    let cases = if quick {
        smoke_cases()
    } else if full {
        full_cases()
    } else {
        standard_cases()
    };

    println!("=== ReelForge e2e bench ===");
    println!(
        "matrix={}  repeats={repeats}  input={}  out={}",
        if quick {
            "quick"
        } else if full {
            "full"
        } else {
            "standard"
        },
        input.as_deref().and_then(Path::to_str).unwrap_or("lavfi"),
        out_dir.display()
    );
    println!(
        "{:<32} {:>10} {:>3} {:>8} / {:>8}     rss      ff     drift   bytes",
        "case", "size", "n", "p50", "p95"
    );

    let mut reports = Vec::new();
    for case in &cases {
        let report = run_e2e_case(case, repeats, &out_dir, input.as_deref())?;
        println!("{}", format_report(&report));
        reports.push(report);
    }

    if json {
        let path = out_dir.join("e2e_report.json");
        std::fs::create_dir_all(&out_dir)?;
        std::fs::write(&path, serde_json::to_string_pretty(&reports)?)?;
        println!("json {}", path.display());
    }
    Ok(())
}

fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2).find_map(|w| {
        if w[0] == name {
            Some(w[1].as_str())
        } else {
            None
        }
    })
}
