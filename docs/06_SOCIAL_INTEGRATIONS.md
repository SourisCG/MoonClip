# 06 — Social Integrations (Zero-Backend)

> Platform-neutral by design: loopback OAuth, system browser, OS keyring and
> clipboard/notification plugins behave the same on Linux and Windows.
> No per-OS code expected here (Windows trip: none).

All uploads are client-to-service. No MoonClip server.

## 1. Google Drive (primary share)

- **Scope (only):** `https://www.googleapis.com/auth/drive.file` — "files created by this app only". Avoids Google security audit, preserves trust.
- **Auth (RFC 8252, desktop):**
  1. Click "Connect Drive" → Rust opens temp loopback `http://127.0.0.1:8989/callback` (`tiny_http`/`warp`).
  2. Open system browser with Google OAuth URL + PKCE S256 (`oauth2` crate).
  3. User consents → Google redirects to loopback with `?code=`.
  4. Rust captures code, replies "Done, close this tab", exchanges for `access_token` (1h) + `refresh_token` (permanent), stores refresh in `keyring`.
- **Upload: resumable (15–200 MB files):**
  1. `POST https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable` with metadata (`name: Clip_2026-09-05_Valorant.mp4`, `mimeType: video/mp4`).
  2. Get `Location:` session URL.
  3. `PUT` chunks 5–10 MB via `reqwest` streaming, emit `%` to frontend for progress bar. Resume from last byte on failure.
- **Medal moment:**
  ```http
  POST /drive/v3/files/{FILE_ID}/permissions
  {"role":"reader","type":"anyone"}
  ```
  Then `GET fields=webViewLink` → copy via `plugin-clipboard-manager` + `plugin-notification` ("Link copied!").
- Crates: `oauth2=4.4`, `tiny_http=0.12`, `reqwest={json,stream}`, `keyring=2` (or `google-drive3` alternative).

## 2. Matrix

| Network | Priority | Flow |
|---|---|---|
| Discord | Essential | Webhook `POST multipart/form-data`. If >25 MB (or >10 MB on old limits), send Drive link instead. Optional FFmpeg fast-compress toggle to fit limit. Zero API cost. |
| YouTube | Essential | Data API v3 `videos.insert`, same Google OAuth. UI selector: Private/Unlisted/Public. Auto-append `#Shorts` if vertical or ≤60s. Quota ~10k units/day (~6 uploads/day per global key); allow advanced users to paste own Client ID. |
| TikTok | Essential | Content Posting API (Direct Post / Inbox Draft). Requires TikTok Developers app + audit. Upload as draft so user adds music. Fallback: open TikTok Studio Web with file ready. |
| Twitter/X | Essential | **No paid API.** Copy file to OS clipboard + open `https://twitter.com/compose/tweet?text=...` (or `twitter.com/intent/tweet?url=<drive>&text=...`). User presses Ctrl+V; video uploads natively. If using Drive link, ensure OpenGraph `twitter:card=player` on viewer page (future web viewer). |
| Instagram/Facebook | Optional | Meta Graph API requires Business/Creator + audit; desktop Reels restricted. Defer past MVP. |

## 3. Acceptance (Phase 6)

- [ ] Drive upload shows live %, finishes with public `webViewLink` copied + notification.
- [ ] Discord webhook sends file or link correctly.
- [ ] Twitter flow opens intent with clipboard ready.
