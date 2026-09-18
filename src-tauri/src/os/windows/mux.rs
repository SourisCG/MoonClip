//! Clip container output: copy-only mux (TS over stdin + solo stems, Mix via
//! `amix`), the §10 track layout, MP4 default-track hardening and the replay
//! file naming.
//!
//! Track layout (order = track number, SPEC §10):
//!
//! - 1: `Master Mix [Game+Voice]` AAC 320k — plays everywhere (default track)
//! - 2: `Game/Desktop` AAC 320k
//! - 3: `Microphone` AAC 192k
//!
//! `audio_single_track` maps only the Master so any player plays it.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

/// Frame rate metadata for the audio tracks (stereo 48 kHz).
pub const MIX_TITLE: &str = "Master Mix [Game+Voice]";
pub const GAME_TITLE: &str = "Game/Desktop";
pub const MIC_TITLE: &str = "Microphone";

/// MIX track from the two solo stems (sample sum, clamped). Stems share the
/// 48 kHz stereo grid and the same window, so they are sample aligned.
pub fn mix_i16(game: &[i16], mic: &[i16]) -> Vec<i16> {
    let n = game.len().min(mic.len());
    game[..n]
        .iter()
        .zip(&mic[..n])
        .map(|(&g, &m)| (g as i32 + m as i32).clamp(i16::MIN as i32, i16::MAX as i32) as i16)
        .collect()
}

/// AAC codec/bitrate/title args for the §10 layout (`single` = Master only).
pub fn audio_args(single: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut flag = |k: &str, v: &str| {
        out.push(k.to_string());
        out.push(v.to_string());
    };
    flag("-c:a", "aac");
    flag("-b:a:0", "320k");
    flag("-metadata:s:a:0", &format!("title={MIX_TITLE}"));
    if !single {
        flag("-b:a:1", "320k");
        flag("-b:a:2", "192k");
        flag("-metadata:s:a:1", &format!("title={GAME_TITLE}"));
        flag("-metadata:s:a:2", &format!("title={MIC_TITLE}"));
    }
    out
}

/// Minimal PCM-16 WAV writer (stereo 48 kHz).
pub fn write_wav(path: &Path, samples: &[i16]) -> std::io::Result<()> {
    use std::io::Write;
    use std::io::BufWriter;
    let data_bytes = (samples.len() * 2) as u32;
    let mut hdr = [0u8; 44];
    hdr[0..4].copy_from_slice(b"RIFF");
    hdr[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    hdr[8..12].copy_from_slice(b"WAVE");
    hdr[12..16].copy_from_slice(b"fmt ");
    hdr[16..20].copy_from_slice(&16u32.to_le_bytes());
    hdr[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    hdr[22..24].copy_from_slice(&2u16.to_le_bytes()); // stereo
    hdr[24..28].copy_from_slice(&48_000u32.to_le_bytes());
    hdr[28..32].copy_from_slice(&(48_000u32 * 2 * 2).to_le_bytes());
    hdr[32..34].copy_from_slice(&4u16.to_le_bytes());
    hdr[34..36].copy_from_slice(&16u16.to_le_bytes());
    hdr[36..40].copy_from_slice(b"data");
    hdr[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    let f = std::fs::File::create(path)?;
    let mut w = BufWriter::with_capacity(1024 * 1024, f);
    w.write_all(&hdr)?;
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    w.write_all(&bytes)?;
    w.flush()
}

/// GSR-style clip name with the chosen container extension.
pub fn replay_filename(container: &str) -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    format!(
        "replay_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}.{}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond, container
    )
}

/// Free destination name: the muxer runs `-y`, so a same-second save must
/// never target a live file.
pub fn unique_dest(dir: &Path, container: &str) -> PathBuf {
    let base = replay_filename(container);
    let stem = base
        .strip_suffix(&format!(".{container}"))
        .unwrap_or(&base)
        .to_string();
    let mut cand = dir.join(&base);
    let mut n = 2u32;
    while cand.exists() {
        cand = dir.join(format!("{stem}_{n}.{container}"));
        n += 1;
    }
    cand
}

/// Mux the staged TS + stems into the final clip. The audio stems are
/// already exactly `window_ns` long starting at the keyframe, so only the
/// video input is seeked (`-ss`) onto that same keyframe: both streams
/// start at 0 aligned, no per-input trims, no re-encode of video.
pub async fn mux_clip(
    ffmpeg: &Path,
    cut_ts: &Path,
    wavs: &[(PathBuf, &str)],
    dest: &Path,
    ss_secs: f64,
    single: bool,
    faststart: bool,
) -> Result<(), String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
    // A hair before the keyframe (input seek drops PTS < target); only
    // applied when the stage actually has pre-roll before the keyframe.
    if ss_secs > 0.001 {
        cmd.args(["-ss", &format!("{ss_secs:.3}")]);
    }
    cmd.arg("-i").arg(cut_ts);
    for (p, _) in wavs {
        cmd.arg("-i").arg(p);
    }
    cmd.args(["-map", "0:v"]);
    for i in 0..wavs.len() {
        cmd.arg("-map").arg(format!("{}:a", i + 1));
    }
    cmd.args(["-c:v", "copy", "-c:a", "aac"]);
    let codecs = audio_args(single);
    let mut ai = 0usize;
    let mut wi = 0usize;
    while wi < wavs.len() {
        cmd.args(["-b:a", if ai == 2 { "192k" } else { "320k" }]);
        cmd.arg(format!("-metadata:s:a:{ai}"))
            .arg(format!("title={}", wavs[wi].1));
        ai += 1;
        wi += 1;
    }
    let _ = codecs;
    if faststart {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.arg("-shortest");
    let out = cmd
        .arg(dest)
        .output()
        .await
        .map_err(|e| format!("save mux failed: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail = err
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!("save mux failed (ffmpeg): {tail}"));
    }
    Ok(())
}

/// Fast mux: TS over stdin, solo stems as WAVs, Mix via `amix`.
#[allow(clippy::too_many_arguments)] // ffmpeg-args builder, kept flat
pub async fn mux_clip_pipe(
    ffmpeg: &Path,
    ts: Vec<u8>,
    game: &Path,
    mic: &Path,
    single: bool,
    dest: &Path,
    container: &str,
    faststart: bool,
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;
    let mut cmd = Command::new(ffmpeg);
    cmd.args([
        "-y", "-hide_banner", "-loglevel", "error",
        "-f", "mpegts", "-i", "pipe:0",
    ]);
    cmd.arg("-i").arg(game).arg("-i").arg(mic);
    cmd.args([
        "-filter_complex",
        "[1:a][2:a]amix=inputs=2:normalize=0:dropout_transition=0[mix]",
        "-map", "0:v", "-map", "[mix]",
    ]);
    if !single {
        cmd.args(["-map", "1:a", "-map", "2:a"]);
    }
    cmd.args(["-c:v", "copy"]);
    cmd.args(audio_args(single));
    if faststart && container == "mp4" {
        cmd.args(["-movflags", "+faststart"]);
    }
    cmd.arg("-shortest")
        .arg(dest)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("save mux failed: {e}"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "mux stdin unavailable".to_string())?;
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&ts).await;
        let _ = stdin.shutdown().await;
    });
    let out = child
        .wait_with_output()
        .await
        .map_err(|e| format!("save mux failed: {e}"))?;
    let _ = writer.await;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail = err
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join(" | ");
        return Err(format!("save mux failed (ffmpeg): {tail}"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// MP4 default-track hardening
// ---------------------------------------------------------------------------

/// Offset of the `alternate_group` u16 inside a `tkhd` payload.
pub(super) fn tkhd_alternate_group_off(version: u8) -> Option<usize> {
    match version {
        0 => Some(34),
        1 => Some(46),
        _ => None,
    }
}

fn box_at(buf: &[u8], off: usize) -> Option<([u8; 4], usize, usize, usize)> {
    if off + 8 > buf.len() {
        return None;
    }
    let size = u32::from_be_bytes(buf[off..off + 4].try_into().ok()?) as u64;
    let mut typ = [0u8; 4];
    typ.copy_from_slice(&buf[off + 4..off + 8]);
    let (header, total) = match size {
        0 => (8u64, (buf.len() - off) as u64),
        1 => {
            if off + 16 > buf.len() {
                return None;
            }
            let large = u64::from_be_bytes(buf[off + 8..off + 16].try_into().ok()?);
            (16, large)
        }
        n => (8, n),
    };
    let total = total as usize;
    if total < header as usize || off + total > buf.len() {
        return None;
    }
    Some((typ, off + header as usize, total - header as usize, off + total))
}

pub(super) fn box_children(buf: &[u8], start: usize, len: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let end = start.saturating_add(len).min(buf.len());
    let mut off = start;
    while off < end {
        let Some((typ, payload, plen, next)) = box_at(buf, off) else {
            break;
        };
        if next <= off {
            break;
        }
        out.push((typ, payload, plen));
        off = next;
        if out.len() > 4096 {
            break;
        }
    }
    out
}

/// `alternate_group` offsets (relative to the `moov` payload) of every audio
/// track. A track counts as audio when its `mdia/hdlr` handler is `soun`.
fn audio_tkhd_alt_group_offsets(moov: &[u8]) -> Vec<usize> {
    let mut offs = Vec::new();
    for (typ, pstart, plen) in box_children(moov, 0, moov.len()) {
        if &typ != b"trak" {
            continue;
        }
        let mut tkhd: Option<usize> = None;
        let mut is_audio = false;
        for (t2, p2, l2) in box_children(moov, pstart, plen) {
            if &t2 == b"tkhd" {
                tkhd = Some(p2);
            } else if &t2 == b"mdia" {
                for (t3, p3, l3) in box_children(moov, p2, l2) {
                    if &t3 == b"hdlr" && l3 >= 12 && &moov[p3 + 8..p3 + 12] == b"soun" {
                        is_audio = true;
                    }
                }
            }
        }
        if !is_audio {
            continue;
        }
        let Some(t) = tkhd else { continue };
        let Some(ver) = moov.get(t).copied() else {
            continue;
        };
        let Some(rel) = tkhd_alternate_group_off(ver) else {
            continue;
        };
        if t + rel + 2 <= moov.len() {
            offs.push(t + rel);
        }
    }
    offs
}

/// Put every audio track of `path` into alternate group `group`: the standard
/// "these tracks are alternatives, use the enabled/default one" signal. Only
/// the 2-byte fields are rewritten in place. Best effort by design.
pub fn patch_audio_alternate_group(path: &Path, group: u16) -> Result<usize, String> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let file_len = f.metadata().map_err(|e| format!("stat: {e}"))?.len();
    let mut off = 0u64;
    let mut moov: Option<(u64, usize)> = None;
    while off + 8 <= file_len {
        f.seek(SeekFrom::Start(off))
            .map_err(|e| format!("seek: {e}"))?;
        let mut hdr = [0u8; 16];
        if f.read(&mut hdr[..8]).map_err(|e| format!("read: {e}"))? < 8 {
            break;
        }
        let size = u32::from_be_bytes(hdr[0..4].try_into().unwrap()) as u64;
        let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
        let (header, total) = match size {
            0 => (8u64, file_len - off),
            1 => {
                f.read_exact(&mut hdr[8..16])
                    .map_err(|e| format!("read largesize: {e}"))?;
                (16u64, u64::from_be_bytes(hdr[8..16].try_into().unwrap()))
            }
            n => (8, n),
        };
        if total < header || off + total > file_len {
            return Err("corrupt mp4 (box size out of range)".into());
        }
        if &typ == b"moov" {
            moov = Some((off + header, (total - header) as usize));
            break;
        }
        off += total;
    }
    let Some((p_off, p_len)) = moov else {
        return Ok(0);
    };
    let mut buf = vec![0u8; p_len];
    f.seek(SeekFrom::Start(p_off))
        .map_err(|e| format!("seek moov: {e}"))?;
    f.read_exact(&mut buf)
        .map_err(|e| format!("read moov: {e}"))?;
    let offsets = audio_tkhd_alt_group_offsets(&buf);
    for rel in &offsets {
        f.seek(SeekFrom::Start(p_off + *rel as u64))
            .map_err(|e| format!("seek field: {e}"))?;
        f.write_all(&group.to_be_bytes())
            .map_err(|e| format!("write field: {e}"))?;
    }
    // Best effort, NO blocking flush: `sync_all` here forced the OS to write
    // back the whole freshly muxed file inside the save path (measured ~2.4 s
    // for a 120 s clip on SATA). The page cache is coherent for the players
    // and the DB/thumbnail work right after; durability comes from the OS.
    let _ = f.sync_all();
    Ok(offsets.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_args_match_spec_layout() {
        let a = audio_args(false);
        let s = a.join(" ");
        assert!(s.contains("-b:a:0 320k"), "{s}");
        assert!(s.contains("-b:a:1 320k"), "{s}");
        assert!(s.contains("-b:a:2 192k"), "{s}");
        assert!(s.contains("title=Master Mix [Game+Voice]"), "{s}");
        assert!(s.contains("title=Game/Desktop"), "{s}");
        assert!(s.contains("title=Microphone"), "{s}");
        // Single-track mode maps only the Master.
        let one = audio_args(true).join(" ");
        assert!(one.contains("-b:a:0 320k"), "{one}");
        assert!(!one.contains("-b:a:1"), "{one}");
        assert!(!one.contains("title=Microphone"), "{one}");
    }

    #[test]
    fn filename_shape_and_dest() {
        let dir = std::env::temp_dir().join(format!("moonclip-mux-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = unique_dest(&dir, "mp4");
        let name = first.file_name().unwrap().to_str().unwrap().to_string();
        assert!(name.starts_with("replay_"), "{name}");
        assert!(name.ends_with(".mp4"), "{name}");
        std::fs::write(&first, b"x").unwrap();
        let second = unique_dest(&dir, "mp4").file_name().unwrap().to_str().unwrap().to_string();
        assert!(second.ends_with("_2.mp4"), "{second}");
        let mkv = unique_dest(&dir, "mkv");
        assert!(mkv.file_name().unwrap().to_str().unwrap().ends_with(".mkv"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mix_sums_aligned_tails() {
        assert_eq!(mix_i16(&[1000, -1000], &[500, 500]), vec![1500, -500]);
        assert_eq!(mix_i16(&[30000, 0], &[30000, 0]), vec![32767, 0]);
        assert!(mix_i16(&[], &[1]).is_empty());
    }

    #[test]
    fn wav_exact_size_and_pcm() {
        let dir = std::env::temp_dir().join(format!("moonclip-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.wav");
        let samples = vec![0i16; 48_000 * 2];
        write_wav(&p, &samples).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes.len(), 44 + 48_000 * 2 * 2);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(i16::from_le_bytes([bytes[44], bytes[45]]), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MP4 hardening: each audio `tkhd` gets `alternate_group` in place,
    /// the video track and the rest stay byte identical.
    #[test]
    fn mp4_patch_audio_alternate_group() {
        fn bx(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
            let mut v = Vec::new();
            v.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
            v.extend_from_slice(typ);
            v.extend_from_slice(payload);
            v
        }
        fn tkhd(version: u8, track_id: u32) -> Vec<u8> {
            let mut p = vec![0u8; if version == 0 { 84 } else { 96 }];
            p[0] = version;
            p[1..4].copy_from_slice(&[0, 0, 3]);
            let id_off = if version == 0 { 12 } else { 20 };
            p[id_off..id_off + 4].copy_from_slice(&track_id.to_be_bytes());
            p
        }
        fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
            let mut p = vec![0u8; 16];
            p[8..12].copy_from_slice(handler);
            p
        }
        fn trak(version: u8, track_id: u32, handler: &[u8; 4]) -> Vec<u8> {
            let mut kids = bx(b"tkhd", &tkhd(version, track_id));
            kids.extend_from_slice(&bx(b"mdia", &bx(b"hdlr", &hdlr(handler))));
            bx(b"trak", &kids)
        }

        let mut moov = Vec::new();
        let video_trak_at = moov.len();
        moov.extend_from_slice(&trak(0, 1, b"vide"));
        let video_alt = video_trak_at + 16 + 34;
        let a0_at = moov.len();
        moov.extend_from_slice(&trak(0, 2, b"soun"));
        let mut expected = vec![a0_at + 16 + 34];
        let a1_at = moov.len();
        moov.extend_from_slice(&trak(1, 3, b"soun"));
        expected.push(a1_at + 16 + 46);
        assert_eq!(audio_tkhd_alt_group_offsets(&moov), expected);

        let mut file = bx(b"ftyp", &[0u8; 16]);
        file.extend_from_slice(&bx(b"mdat", &[0u8; 32]));
        let moov_payload = file.len() + 8;
        file.extend_from_slice(&bx(b"moov", &moov));
        let before = file.clone();
        let path = std::env::temp_dir().join(format!(
            "moonclip-mp4-group-{}.mp4",
            std::process::id()
        ));
        std::fs::write(&path, &file).unwrap();

        assert_eq!(patch_audio_alternate_group(&path, 1).unwrap(), 2);
        let after = std::fs::read(&path).unwrap();
        for (i, rel) in expected.iter().enumerate() {
            let at = moov_payload + rel;
            assert_eq!(&after[at..at + 2], &[0, 1], "audio track {i} not patched");
        }
        let vat = moov_payload + video_alt;
        assert_eq!(&after[vat..vat + 2], &[0, 0], "video track was patched");
        let diffs = before
            .iter()
            .zip(&after)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let mut want: Vec<usize> = expected.iter().map(|r| moov_payload + r + 1).collect();
        want.sort_unstable();
        assert_eq!(diffs, want, "unexpected bytes changed");
        let _ = std::fs::remove_file(&path);
    }
}
