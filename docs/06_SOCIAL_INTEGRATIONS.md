# 06 — Social Integrations (Zero-Backend)

> Platform-neutral by design: loopback OAuth, system browser, OS keyring and
> clipboard/notification plugins behave the same on Linux and Windows.
> No per-OS code expected here (Windows trip: none).
>
> **No secrets in git.** The repository is public: `social.json`, client
> secrets, tokens and webhook URLs never get committed (see
> `05_STORAGE_SECURITY.md` §Secrets policy and `scripts/check-secrets.sh`).

All uploads go client-to-service. MoonClip operates no backend that receives
user data; the only server component is a stateless Cloudflare Worker that adds
the TikTok client secret during token exchange/refresh.

## 0. Status (2026-09-28)

- **Google Drive: implemented.** Loopback OAuth (ephemeral port, PKCE S256 +
  `state`, 3 min timeout) against the `moonclip-drive` project's Desktop client
  (`drive.file` + `userinfo.email`); tokens as a JSON blob in the keyring with
  automatic refresh; root folder `MoonClip` and mirror `MoonClip/<game>/` with a
  cached mapping in `drive_folders` (migration 014). Resumable uploads in 8 MiB
  chunks with progress, exact resume from the 308 `Range` and one transport
  retry; remote name conflict → `_2`. **Private by default**; the "make public"
  toggle creates the anyone-reader permission and returns `webViewLink` to copy.
  Downloads stream into the local game folder (thumbnail + probe + row +
  `clip-saved`); local conflict → `_2`. UI: Settings → Accounts (connect /
  disconnect), a share button per clip and the Drive browser in the gallery.
- **Credentials**: `social.json` in the app-data dir (outside git, `0600`) with
  env overrides (`MOONCLIP_GOOGLE_DRIVE_CLIENT_ID`/`_SECRET`); template in
  `social.example.json`. The keyring is never exposed over IPC.
- **Cloud clips**: after an upload, the optional "delete the local video"
  checkbox leaves the clip only in Drive (local thumbnail kept, row `cloud = 1`,
  `drive_file_id`). Preview/trim use `ensure_local` (download to the app cache
  with progress; the trim panel drops the copy on close); the heavy editor
  always downloads into its session dir (cleaned on close) and the export reuses
  that copy. Re-uploading a cloud clip reuses the remote file (no duplicates).
  Deleting a cloud clip sends the remote file to the Drive trash (two-step
  confirmation); purge and quota ignore cloud rows. Migration 015.
- **Consistent uploads**: `drive_file_id`/link is stored on EVERY upload (not
  only when deleting the local copy); re-uploading reuses the remote file
  (verifying it is alive; trashed/404 → cleared and uploaded again) and
  "Replace in Drive" trashes the old one. The share dialog tracks
  pending/uploaded state (copy link, make public, open in Drive, delete local
  now, replace).
- **Library restore**: connecting Drive on another machine rebuilds the gallery
  from the remote tree: already-uploaded videos are indexed as **cloud-only**
  rows (`cloud = 1`, ids, duration from `videoMediaMetadata`, original
  `created_at` normalized) with the thumbnail Drive generates (`thumbnailLink`)
  and the folder mapping reused (`drive_folders`). Automatic on connect plus a
  "Restore library" button in Settings → Accounts; opening/editing downloads on
  demand (`ensure_local`). The sync is idempotent (match by `drive_file_id`,
  `_2` names), runs one at a time and self-heals missing thumbnails on every
  pass.
- **Medal-style navigation**: the gallery is part of the Medal shell (icon rail +
  context rail + header, see `07_UI_MOONCLIP.md`); filters are "All, Favorites,
  Uploaded, In Drive" plus the game list with counts. Clicking a card opens the
  clip panel (player, metadata, inline rename, actions: share, editor,
  favorite, reveal, delete with cloud confirmation, ←/→ navigation and Esc);
  cloud clips download on demand with % and the temporary copy is deleted on
  close. Loop playback is opt-in (stops at the end by default).
- **YouTube (code complete)**: same Google loopback OAuth as Drive
  (`youtube.upload` scope, own keyring token; falls back to the Drive client
  when `google_youtube` is absent from `social.json`). Resumable
  `videos.insert` (8 MiB chunks, resume from the 308 `Range`, progress over
  `moonclip://publish-progress`), user title + fixed description
  `#MoonClip #moonclip`, Gaming category, Private/Unlisted/Public privacy
  (default private) and the community-guidelines confirmation checkbox.
  Upload-only: no browsing, no downloads. **External gate**: until the YouTube
  API compliance audit (and the sensitive-scope verification) passes, every API
  upload is locked private; the UI says so (paperwork in the PROGRESS log).
- **Discord**: "connect your account" with no bot and no backend — OAuth
  `webhook.incoming` with Public Client + PKCE S256 (no client secret) over the
  fixed loopback `127.0.0.1:38471`; the user picks a server + channel in the
  browser and the returned webhook (id/token/url) is stored in the keyring with
  the OAuth grant revoked immediately. Streaming multipart upload with
  `?wait=true` and progress (`moonclip://publish-progress`, phases
  compress/upload), `payload_json` = title + `#MoonClip #moonclip` with mentions
  disabled; 413/429/401-404 map to actionable errors. Clips over the server
  limit (`discord_max_mb`: 10/25/50/100, default 10) offer aggressive 720p
  compression (CRF 30, then a computed bitrate if it still does not fit, AAC
  96k, one track) to a temp file that is always deleted. Disconnect deletes the
  webhook from the channel. MoonClip's app is embedded as the public client id
  (override in `social.json` / `MOONCLIP_DISCORD_CLIENT_ID`).
- **TikTok**: desktop Login Kit (ephemeral loopback
  `http://127.0.0.1:<port>/callback/` + PKCE **hex** SHA256 — TikTok does not use
  base64url) and Content Posting API Direct Post with `FILE_UPLOAD` in 10 MiB
  chunks + status polling. Token exchange/refresh goes through the Cloudflare
  Worker (`workers/moonclip-oauth/`, `/tiktok/exchange` and `/tiktok/refresh`)
  so the `client_secret` never ships in the app; a direct mode with a local
  secret in `social.json` exists as a fallback. The UI reads `creator_info` and
  renders the privacy dropdown with **no default value** (TikTok's rule),
  honors the creator's comment/duet/stitch settings and validates the maximum
  duration before uploading. While unaudited, posts are `SELF_ONLY` and the
  account must be private. Rotating refresh tokens in the keyring
  (`oauth_tiktok`).
- Pending: Twitter/X (browser + file clipboard).

## 1. Google Drive (primary share)

- **Scope (only):** `https://www.googleapis.com/auth/drive.file` — "files
  created by this app only". Avoids Google's security audit and preserves trust.
- **Auth (RFC 8252, desktop):**
  1. "Connect Drive" → Rust starts an ephemeral loopback server
     (`http://127.0.0.1:<port>/callback`, `tiny_http`).
  2. The system browser opens the Google consent URL with PKCE S256 + `state`
     (hand-rolled in `social/oauth.rs`).
  3. The user consents → Google redirects to the loopback with `?code=`.
  4. Rust validates `state`, answers the browser ("Done, close this tab") and
     exchanges the code for `access_token` + `refresh_token` (vaulted).
- **Upload: resumable (15–200 MB files):**
  1. `POST https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable`
     with metadata (`name`, `mimeType: video/mp4`, `parents`).
  2. Read the `Location:` session URL.
  3. `PUT` 8 MiB chunks via `reqwest` streaming, emitting progress to the
     frontend; exact resume from the server-reported `Range` after a 308.
- **Public link:**
  ```http
  POST /drive/v3/files/{FILE_ID}/permissions
  {"role":"reader","type":"anyone"}
  ```
  Then `GET fields=webViewLink` → copy via `plugin-clipboard-manager` +
  `plugin-notification`.
- Crates: `tiny_http`, `reqwest` (`json`, `stream`), `sha2`, `base64`,
  `urlencoding`, `keyring`.

## 2. Matrix

| Network | Priority | Flow |
|---|---|---|
| Discord | Essential | OAuth `webhook.incoming` (Public Client + PKCE, no bot, no backend): the user picks a server + channel and the webhook lands in the keyring. `POST ?wait=true` multipart with streaming progress. Over-limit clips compress to a temporary 720p copy (optional; never a Drive-link fallback). Zero API cost. |
| YouTube | Essential | Data API v3 `videos.insert` over the same Google OAuth (implemented). Privacy selector: Private/Unlisted/Public. Fixed description `#MoonClip #moonclip`; default quota 100 `videos.insert`/day per project. Without the compliance audit, API uploads are locked private. |
| TikTok | Essential | Desktop Login Kit (loopback + PKCE, TikTok's hex S256) + Content Posting API Direct Post (`FILE_UPLOAD`, 10 MiB chunks, status polling). A Cloudflare Worker brokers token exchange/refresh so the secret never ships. Privacy dropdown rendered from `creator_info` with **no default** (TikTok rule); unaudited clients post `SELF_ONLY` to private accounts. |
| Twitter/X | Essential | **No paid API.** Copy file to OS clipboard + open `https://twitter.com/compose/tweet?text=...` (or `twitter.com/intent/tweet?url=<drive>&text=...`). User presses Ctrl+V; video uploads natively. If using a Drive link, ensure OpenGraph `twitter:card=player` on the viewer page (future web viewer). |
| Instagram/Facebook | Optional | Meta Graph API requires Business/Creator + audit; desktop Reels restricted. Defer past MVP. |

## 3. Acceptance (Phase 6)

- [x] Drive upload shows live %, finishes with public `webViewLink` copied + notification.
- [x] Drive library restore on a second machine (cloud rows with thumbnails).
- [ ] YouTube upload shows live %, finishes with the video link (public visibility waits for the project audit).
- [ ] Discord: connect flow captures the webhook; the upload sends the file
      (or the compressed copy) with live progress (code + mock tests green;
      owner live pass pending).
- [ ] TikTok: sandbox connect + publish shows live % and completes (code +
      mock tests green; owner live pass pending).
- [ ] Twitter flow opens intent with clipboard ready.

## 4. Secrets

Already said at the top and in `05_STORAGE_SECURITY.md`, and it will be said
again: **never commit secrets**. Google Desktop client ids are public
identifiers; client secrets, refresh/access tokens and webhook URLs are not.
The TikTok client secret lives only in the Cloudflare Worker
(`wrangler secret put`), never in the repository.
