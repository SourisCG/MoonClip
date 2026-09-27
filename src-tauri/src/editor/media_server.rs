//! Loopback HTTP media server for in-app playback.
//!
//! WebKitGTK cannot play audio/video from Tauri's `asset://` custom scheme
//! (WebKit bug 146351; still reproducible on Fedora 44 + WebKitGTK 2.5x), so
//! media elements get a `http://127.0.0.1:<port>/m/<token>` URL instead.
//! Windows could use `asset://`, but one path for both platforms is simpler
//! and the server is only started on first use (never at boot).
//!
//! Security: binds 127.0.0.1 with an ephemeral port, serves ONLY absolute
//! paths explicitly registered by the backend (random unguessable token per
//! file), supports GET/HEAD with HTTP range requests (required for seeking
//! and for MP4 files whose `moov` atom sits at the end).

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone)]
struct MediaEntry {
    session: Option<String>,
    path: PathBuf,
}

struct MediaServer {
    port: u16,
    files: Arc<Mutex<HashMap<String, MediaEntry>>>,
}

static SERVER: OnceLock<MediaServer> = OnceLock::new();
static INIT: Mutex<()> = Mutex::new(());

/// URL the webview can play for `path` (starts the server on first call).
pub fn media_url(path: &Path) -> Result<String, String> {
    media_url_session(None, path)
}

/// Session-scoped URL: `release_session` removes every token it registered
/// (editor sessions are fully torn down on close).
pub fn media_url_session(session: Option<&str>, path: &Path) -> Result<String, String> {
    let server = ensure_server()?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    server
        .files
        .lock()
        .map_err(|_| "media server lock poisoned".to_string())?
        .insert(
            token.clone(),
            MediaEntry {
                session: session.map(str::to_string),
                path: path.to_path_buf(),
            },
        );
    Ok(format!("http://127.0.0.1:{}/m/{}", server.port, token))
}

/// Drop every media token belonging to a session (editor close).
pub fn release_session(session: &str) {
    if let Some(server) = SERVER.get() {
        if let Ok(mut files) = server.files.lock() {
            files.retain(|_, e| e.session.as_deref() != Some(session));
        }
    }
}

fn ensure_server() -> Result<&'static MediaServer, String> {
    if let Some(s) = SERVER.get() {
        return Ok(s);
    }
    let _guard = INIT.lock().map_err(|_| "media init lock poisoned".to_string())?;
    if let Some(s) = SERVER.get() {
        return Ok(s);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .map_err(|e| format!("cannot bind media server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read media server port: {e}"))?
        .port();
    let files: Arc<Mutex<HashMap<String, MediaEntry>>> = Arc::new(Mutex::new(HashMap::new()));
    let files_thread = files.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let files = files_thread.clone();
            std::thread::spawn(move || {
                let _ = handle_connection(stream, &files);
            });
        }
    });
    let server = SERVER.get_or_init(|| MediaServer { port, files });
    eprintln!("[moonclip] media server on 127.0.0.1:{port}");
    Ok(server)
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("mp4") | Some("m4v") | Some("mov") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("m4a") | Some("aac") | Some("m4b") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") | Some("opus") => "audio/ogg",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

/// `Range: bytes=a-b` / `bytes=a-` / `bytes=-n` -> (start, end_inclusive-ish).
fn parse_range(value: &str, total: u64) -> Result<(u64, u64), ()> {
    let spec = value.trim().strip_prefix("bytes=").ok_or(())?;
    if total == 0 || spec.contains(',') {
        return Err(());
    }
    let (a, b) = spec.split_once('-').ok_or(())?;
    let (start, end) = if a.is_empty() {
        let suffix: u64 = b.trim().parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (total.saturating_sub(suffix), total - 1)
    } else {
        let start: u64 = a.trim().parse().map_err(|_| ())?;
        let end = if b.trim().is_empty() {
            total - 1
        } else {
            b.trim().parse::<u64>().map_err(|_| ())?.min(total - 1)
        };
        (start, end)
    };
    if start > end || start >= total {
        return Err(());
    }
    Ok((start, end))
}

fn handle_connection(
    mut stream: TcpStream,
    files: &Arc<Mutex<HashMap<String, MediaEntry>>>,
) -> std::io::Result<()> {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request = String::new();
    if reader.read_line(&mut request)? == 0 {
        return Ok(());
    }
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let mut range_header: Option<String> = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("range:") {
            range_header = Some(v.trim().to_string());
        }
    }
    // Loopback CORS so the webview can `fetch` (wavesurfer decodes peaks with
    // fetch; <video>/<audio> do not need it but it costs nothing).
    let respond = |stream: &mut TcpStream, status: &str, headers: &[(String, String)]| {
        let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        head.push_str("Access-Control-Allow-Origin: *\r\n");
        head.push_str("Access-Control-Allow-Methods: GET, HEAD, OPTIONS\r\n");
        head.push_str("Access-Control-Allow-Headers: Range\r\n");
        head.push_str("Access-Control-Expose-Headers: Content-Range, Content-Length, Accept-Ranges\r\n");
        for (k, v) in headers {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())
    };

    if method == "OPTIONS" {
        respond(&mut stream, "204 No Content", &[])?;
        return Ok(());
    }
    if method != "GET" && method != "HEAD" {
        respond(&mut stream, "405 Method Not Allowed", &[])?;
        return Ok(());
    }
    let token = target
        .strip_prefix("/m/")
        .map(|t| t.split(['?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    let path = files.lock().ok().and_then(|m| m.get(token).map(|e| e.path.clone()));
    let Some(path) = path else {
        respond(&mut stream, "404 Not Found", &[])?;
        return Ok(());
    };
    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            respond(&mut stream, "500 Internal Server Error", &[("X-Error".into(), e.to_string())])?;
            return Ok(());
        }
    };
    let total = file.metadata()?.len();
    let mime = mime_for(&path).to_string();
    let range = range_header.as_deref().map(|h| parse_range(h, total));
    match range {
        Some(Err(())) => {
            respond(
                &mut stream,
                "416 Range Not Satisfiable",
                &[("Content-Range".into(), format!("bytes */{total}"))],
            )?;
        }
        Some(Ok((start, end))) => {
            let len = end - start + 1;
            respond(
                &mut stream,
                "206 Partial Content",
                &[
                    ("Content-Type".into(), mime),
                    ("Content-Length".into(), len.to_string()),
                    ("Accept-Ranges".into(), "bytes".into()),
                    ("Content-Range".into(), format!("bytes {start}-{end}/{total}")),
                    ("Cache-Control".into(), "private, max-age=86400".into()),
                ],
            )?;
            if method == "GET" {
                file.seek(SeekFrom::Start(start))?;
                std::io::copy(&mut file.take(len), &mut stream)?;
            }
        }
        None => {
            respond(
                &mut stream,
                "200 OK",
                &[
                    ("Content-Type".into(), mime),
                    ("Content-Length".into(), total.to_string()),
                    ("Accept-Ranges".into(), "bytes".into()),
                    ("Cache-Control".into(), "private, max-age=86400".into()),
                ],
            )?;
            if method == "GET" {
                std::io::copy(&mut file, &mut stream)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HttpReply {
        status: u16,
        headers: String,
        body: Vec<u8>,
    }

    fn request(addr: &str, raw: &str) -> HttpReply {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream.write_all(raw.as_bytes()).unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        let split = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("headers end");
        let head = String::from_utf8_lossy(&buf[..split]).to_string();
        let status = head
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        HttpReply {
            status,
            headers: head.to_ascii_lowercase(),
            body: buf[split + 4..].to_vec(),
        }
    }

    #[test]
    fn serves_full_and_range_requests() {
        let dir = std::env::temp_dir().join(format!("moonclip-media-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.mp4");
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&file, &data).unwrap();

        let url = media_url(&file).unwrap();
        assert!(url.starts_with("http://127.0.0.1:"));
        let rest = url.trim_start_matches("http://");
        let (addr, path) = rest.split_once('/').unwrap();

        let full = request(addr, &format!("GET /{path} HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert_eq!(full.status, 200, "headers: {}", full.headers);
        assert!(full.headers.contains("accept-ranges: bytes"));
        assert!(full.headers.contains("content-type: video/mp4"));
        assert_eq!(full.body, data);

        let head = request(addr, &format!("HEAD /{path} HTTP/1.1\r\nHost: x\r\n\r\n"));
        assert_eq!(head.status, 200);
        assert!(head.body.is_empty());

        let part = request(
            addr,
            &format!("GET /{path} HTTP/1.1\r\nHost: x\r\nRange: bytes=100-199\r\n\r\n"),
        );
        assert_eq!(part.status, 206);
        assert!(part.headers.contains("content-range: bytes 100-199/1000"));
        assert_eq!(part.body, data[100..200]);

        let suffix = request(
            addr,
            &format!("GET /{path} HTTP/1.1\r\nHost: x\r\nRange: bytes=-50\r\n\r\n"),
        );
        assert_eq!(suffix.status, 206);
        assert_eq!(suffix.body, data[950..1000]);

        let bad = request(
            addr,
            &format!("GET /{path} HTTP/1.1\r\nHost: x\r\nRange: bytes=5000-6000\r\n\r\n"),
        );
        assert_eq!(bad.status, 416);

        let missing = request(addr, "GET /m/nope HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(missing.status, 404);

        // The webview fetches these URLs (wavesurfer): CORS must be present.
        assert!(full.headers.contains("access-control-allow-origin: *"));
        let preflight = request(addr, "OPTIONS /m/x HTTP/1.1\r\nHost: x\r\n\r\n");
        assert_eq!(preflight.status, 204);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
