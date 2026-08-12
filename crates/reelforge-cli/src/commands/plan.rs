//! `reelforge plan` — inspect / optimize / run a JSON `RenderPlan`.

use reelforge::{RenderPlan, explain_plan, extract_ffmpeg, optimize_plan, run_render_plan};
use std::path::Path;

/// Plan subcommand mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanMode {
    /// Print extraction summary.
    Explain,
    /// Write optimized JSON to stdout or `--out`.
    Optimize,
    /// Write extraction JSON (ffmpeg prefix + remainder).
    Extract,
    /// Execute plan (pure `FFmpeg` or hybrid FFmpeg+Rust).
    Run,
}

/// Load `path`, then explain / optimize / extract / run.
///
/// # Errors
///
/// Returns string errors for I/O, parse, or execution failures.
pub fn run(path: &str, mode: PlanMode, out: Option<&str>) -> Result<(), String> {
    let plan = RenderPlan::load(path).map_err(|e| e.to_string())?;
    match mode {
        PlanMode::Explain => {
            println!("{}", explain_plan(&plan));
            Ok(())
        }
        PlanMode::Optimize => {
            let optimized = optimize_plan(&plan);
            let text = optimized.plan.to_json_pretty().map_err(|e| e.to_string())?;
            write_or_print(out, &text)?;
            eprintln!(
                "optimize: {} → {} ops (eliminated {})",
                optimized.stats.before,
                optimized.stats.after,
                optimized.stats.eliminated()
            );
            Ok(())
        }
        PlanMode::Extract => {
            let extracted = extract_ffmpeg(&plan);
            let text = serde_json::to_string_pretty(&extracted).map_err(|e| e.to_string())?;
            write_or_print(out, &text)?;
            eprintln!(
                "extract: fully_ffmpeg={} ffmpeg_ops={} remainder={}",
                extracted.fully_ffmpeg, extracted.ffmpeg_op_count, extracted.remainder_op_count
            );
            Ok(())
        }
        PlanMode::Run => {
            run_render_plan(&plan).map_err(|e| e.to_string())?;
            if let Some(output) = plan.output.as_ref() {
                println!("wrote {}", output.path);
            }
            Ok(())
        }
    }
}

fn write_or_print(out: Option<&str>, text: &str) -> Result<(), String> {
    match out {
        Some(path) => {
            if let Some(parent) = Path::new(path).parent()
                && !parent.as_os_str().is_empty()
                && !parent.exists()
            {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(path, text).map_err(|e| e.to_string())?;
            println!("wrote {path}");
        }
        None => print!("{text}"),
    }
    Ok(())
}
