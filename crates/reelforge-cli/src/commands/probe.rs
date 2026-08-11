//! `reelforge probe`

use reelforge::{OpenVideoOptions, VideoClip, open_video};

/// Probe a video file and print basic metadata.
///
/// # Errors
///
/// Returns a string error when tools or the file are unavailable.
pub fn run(path: &str) -> Result<(), String> {
    let clip = open_video(&OpenVideoOptions::new(path)).map_err(|e| e.to_string())?;
    println!("path: {}", clip.path().display());
    println!("size: {}x{}", clip.size().width, clip.size().height);
    println!("duration: {:.3}s", clip.duration().as_secs());
    if let Some(fps) = clip.fps() {
        println!("fps: {fps:.3}");
    }
    Ok(())
}
