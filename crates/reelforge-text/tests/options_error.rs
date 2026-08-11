//! Options and error surface coverage.

use reelforge_core::{Duration, Rgba8, Size, VideoClip};
use reelforge_text::{TextClipOptions, TextError, text_clip};

#[test]
fn options_builders() {
    let o = TextClipOptions::new("Title", 24, Duration::from_secs(2.0))
        .with_color(Rgba8::WHITE)
        .with_size(Size::new(200, 80))
        .with_padding(8)
        .with_font_path("bitmap");
    assert_eq!(o.padding, 8);
    assert_eq!(o.size, Some(Size::new(200, 80)));
    let clip = text_clip(&o).unwrap();
    assert_eq!(clip.size(), Size::new(200, 80));
}

#[test]
fn error_display() {
    let e = TextError::font("missing");
    assert!(e.to_string().contains("font"));
    let e = TextError::layout("bad");
    assert!(e.to_string().contains("layout"));
}
