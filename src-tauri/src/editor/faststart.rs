//! Faststart copies for playback.
//!
//! OBS writes the MP4 `moov` atom at the END of the file, so a player must
//! fetch the tail before it can show the first frame (seconds of black on
//! big clips). A one-time remux with `-c copy` (no transcode, no GPU) moves
//! the index to the front; the copy is cached and served by the media URL.

use std::path::{Path, PathBuf};

/// Does the MP4 put its index (`moov`) before the media data (`mdat`)?
/// Only the head is scanned: faststart files always have `moov` first.
pub fn is_faststart(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = vec![0u8; 2 * 1024 * 1024];
    let Ok(n) = file.read(&mut head) else {
        return false;
    };
    let head = &head[..n];
    let find = |needle: &[u8]| head.windows(needle.len()).position(|w| w == needle);
    match (find(b"moov"), find(b"mdat")) {
        (Some(moov), Some(mdat)) => moov < mdat,
        (Some(_), None) => true, // moov first, mdat later (or fragmented)
        _ => false,
    }
}

/// Cached faststart copy of `src`, created on demand. Falls back to `src`
/// when the remux fails (playback must never break because of this).
pub async fn ensure_faststart_copy(
    ffmpeg: &Path,
    cache_root: &Path,
    clip_id: &str,
    src: &Path,
) -> PathBuf {
    if is_faststart(src) {
        return src.to_path_buf();
    }
    let dir = cache_root.join("faststart");
    let dest = dir.join(format!("{clip_id}.mp4"));
    if dest.is_file() {
        return dest;
    }
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        eprintln!("[moonclip] faststart: cannot create cache: {e}");
        return src.to_path_buf();
    }
    let part = dir.join(format!("{clip_id}.part.mp4"));
    let _ = tokio::fs::remove_file(&part).await;
    let output = tokio::process::Command::new(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(src)
        .args(["-map", "0", "-c", "copy", "-movflags", "+faststart"])
        .arg(&part)
        .output()
        .await;
    match output {
        Ok(o) if o.status.success() => {
            if let Err(e) = tokio::fs::rename(&part, &dest).await {
                eprintln!("[moonclip] faststart: cannot finalize the copy: {e}");
                return src.to_path_buf();
            }
            eprintln!(
                "[moonclip] faststart copy ready: {} -> {}",
                src.display(),
                dest.display()
            );
            dest
        }
        Ok(o) => {
            eprintln!(
                "[moonclip] faststart remux failed ({}); serving the original",
                String::from_utf8_lossy(&o.stderr).trim()
            );
            let _ = tokio::fs::remove_file(&part).await;
            src.to_path_buf()
        }
        Err(e) => {
            eprintln!("[moonclip] faststart remux could not run: {e}");
            src.to_path_buf()
        }
    }
}

/// Drop the faststart cache (app start: it is recreated per session).
pub fn cleanup_cache(cache_root: &Path) {
    let dir = cache_root.join("faststart");
    if dir.is_dir() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_mp4(dir: &Path, name: &str, atoms: &[&[u8]]) -> PathBuf {
        let path = dir.join(name);
        let mut bytes = Vec::new();
        for atom in atoms {
            bytes.extend_from_slice(atom);
        }
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn detects_moov_position() {
        let dir = std::env::temp_dir().join(format!(
            "moonclip-faststart-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // OBS-style: mdat first, moov at the end.
        let late = write_mp4(&dir, "late.mp4", &[b"ftypmdat", b"....", b"moov"]);
        assert!(!is_faststart(&late));
        // Remuxed: moov right after ftyp.
        let early = write_mp4(&dir, "early.mp4", &[b"ftypmoov", b"....", b"mdat"]);
        assert!(is_faststart(&early));
        // Fragmented/only-moov-in-head counts as faststart.
        let only = write_mp4(&dir, "only.mp4", &[b"ftypmoov"]);
        assert!(is_faststart(&only));
        assert!(!is_faststart(&dir.join("missing.mp4")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live: a non-faststart file gets a copy whose moov is at the front.
    #[tokio::test]
    async fn live_remux_moves_moov_to_the_front() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ff = manifest
            .join("binaries")
            .join(crate::sidecar::host_triple())
            .join(crate::editor::ffmpeg::sidecar_name());
        if !ff.exists() {
            eprintln!("skip: bundled ffmpeg not staged");
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "moonclip-faststart-live-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.mp4");
        let ok = tokio::process::Command::new(&ff)
            .args([
                "-y", "-hide_banner", "-loglevel", "error",
                "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=30",
                "-t", "1", "-c:v", "libx264", "-preset", "ultrafast",
            ])
            .arg(&src)
            .status()
            .await
            .unwrap();
        assert!(ok.success(), "fixture encode failed");
        // Default MP4 muxer writes moov at the end unless +faststart is asked.
        assert!(!is_faststart(&src), "fixture unexpectedly faststart");
        let copy = ensure_faststart_copy(&ff, &dir, "clip-1", &src).await;
        assert_ne!(copy, src);
        assert!(is_faststart(&copy), "remuxed copy must be faststart");
        // Second call reuses the cache (no new file needed).
        let again = ensure_faststart_copy(&ff, &dir, "clip-1", &src).await;
        assert_eq!(again, copy);
        cleanup_cache(&dir);
        assert!(!dir.join("faststart").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
