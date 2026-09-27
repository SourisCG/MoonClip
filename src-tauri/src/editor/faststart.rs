//! Faststart copies for playback.
//!
//! OBS writes the MP4 `moov` atom at the END of the file, so a player must
//! fetch the tail before it can show the first frame (seconds of black on
//! big clips). A one-time remux with `-c copy` (no transcode, no GPU) moves
//! the index to the front; the copy is cached and served by the media URL.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Per-clip single-flight lock: two concurrent player opens (e.g. React
/// StrictMode effects) must not remux the same clip twice or race on the
/// `.part` file (observed: one renamed it, the other failed with ENOENT).
fn clip_locks() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn clip_lock(clip_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    clip_locks()
        .lock()
        .map(|mut map| {
            map.entry(clip_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        })
        .unwrap_or_else(|_| Arc::new(tokio::sync::Mutex::new(())))
}

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
    // Single-flight: the second caller waits and then finds the cached copy.
    let lock = clip_lock(clip_id);
    let _guard = lock.lock().await;
    if dest.is_file() {
        return dest;
    }
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        eprintln!("[moonclip] faststart: cannot create cache: {e}");
        return src.to_path_buf();
    }
    // Unique part name: a stale part from a crashed run can never collide.
    let part = dir.join(format!(
        "{clip_id}.{}.part.mp4",
        uuid::Uuid::new_v4().simple()
    ));
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
                // A concurrent process may have won the race (or the cache
                // was wiped mid-flight); never fall back when dest is there.
                if dest.is_file() {
                    return dest;
                }
                eprintln!("[moonclip] faststart: cannot finalize the copy: {e}");
                let _ = tokio::fs::remove_file(&part).await;
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
            let _ = tokio::fs::remove_file(&part).await;
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

    /// Bundled ffmpeg when staged, else None (live tests skip).
    fn live_ffmpeg() -> Option<PathBuf> {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let ff = manifest
            .join("binaries")
            .join(crate::sidecar::host_triple())
            .join(crate::editor::ffmpeg::sidecar_name());
        ff.exists().then_some(ff)
    }

    /// Non-faststart MP4 fixture (the default muxer writes moov at the end).
    async fn encode_fixture(ff: &Path, dir: &Path) -> PathBuf {
        let src = dir.join("src.mp4");
        let ok = tokio::process::Command::new(ff)
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
        assert!(!is_faststart(&src), "fixture unexpectedly faststart");
        src
    }

    /// Live: a non-faststart file gets a copy whose moov is at the front.
    #[tokio::test]
    async fn live_remux_moves_moov_to_the_front() {
        let Some(ff) = live_ffmpeg() else {
            eprintln!("skip: bundled ffmpeg not staged");
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "moonclip-faststart-live-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = encode_fixture(&ff, &dir).await;
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

    /// Live regression: two concurrent player opens for the same clip used to
    /// race on the shared `.part` file (one rename won, the other failed with
    /// ENOENT and fell back to the slow original). Both must return the same
    /// faststart copy now.
    #[tokio::test]
    async fn concurrent_calls_share_one_copy() {
        let Some(ff) = live_ffmpeg() else {
            eprintln!("skip: bundled ffmpeg not staged");
            return;
        };
        let dir = std::env::temp_dir().join(format!(
            "moonclip-faststart-race-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src = encode_fixture(&ff, &dir).await;
        let (a, b) = tokio::join!(
            ensure_faststart_copy(&ff, &dir, "clip-race", &src),
            ensure_faststart_copy(&ff, &dir, "clip-race", &src),
        );
        assert_eq!(a, b, "both callers must get the same cached copy");
        assert!(is_faststart(&a));
        // No leftover parts.
        let parts: Vec<_> = std::fs::read_dir(dir.join("faststart"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".part."))
            .collect();
        assert!(parts.is_empty(), "no .part files may survive");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
