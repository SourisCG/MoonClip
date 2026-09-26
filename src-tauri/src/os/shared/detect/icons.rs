//! Icon caching: normalize any source image into a stable PNG under the
//! MoonClip data dir (used by the Games UI; never uploaded anywhere).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

/// Copy/convert `src` into `<icons_dir>/<hash>.png`. Same bytes produce the
/// same file name, so repeated resolution is a no-op after the first time.
pub fn cache_icon(src: &Path, icons_dir: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(src).ok()?;
    let image = image::load_from_memory(&bytes).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let name = format!("{:016x}.png", hasher.finish());
    std::fs::create_dir_all(icons_dir).ok()?;
    let out = icons_dir.join(name);
    if !out.exists() {
        image.save(&out).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn caches_and_dedupes_by_content() {
        let tmp = std::env::temp_dir().join(format!("moonclip-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let src = tmp.join("src.jpg");
        std::fs::write(&src, png_bytes()).unwrap();
        let icons = tmp.join("icons");

        let a = cache_icon(&src, &icons).unwrap();
        let b = cache_icon(&src, &icons).unwrap();
        assert_eq!(a, b, "same bytes must map to the same cached file");
        assert!(a.is_file());
        assert_eq!(a.extension().and_then(|s| s.to_str()), Some("png"));

        assert!(cache_icon(&tmp.join("missing.png"), &icons).is_none());
        std::fs::write(&src, b"not an image").unwrap();
        assert!(cache_icon(&src, &icons).is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
