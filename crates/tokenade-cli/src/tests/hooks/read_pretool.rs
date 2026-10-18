//! Tests for the `read_pretool` hook.

use super::*;

#[test]
fn is_image_path_matches_raster_only() {
    for p in [
        "/a/slide-11.jpg",
        "shot.JPEG",
        "/tmp/x.png",
        "anim.gif",
        "pic.webp",
        "scan.TIFF",
    ] {
        assert!(is_image_path(p), "should be image: {p}");
    }
    // SVG is text/XML → NOT a vision block; goes through the text path.
    // Code/data/docs are never image-deduped.
    for p in [
        "icon.svg",
        "main.rs",
        "data.json",
        "report.pdf",
        "notes.md",
        "noext",
    ] {
        assert!(!is_image_path(p), "should NOT be image: {p}");
    }
}
