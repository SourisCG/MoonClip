# 06 — Social Integrations (Zero-Backend)

> Platform-neutral by design: loopback OAuth, system browser, OS keyring and
> clipboard/notification plugins behave the same on Linux and Windows.
> No per-OS code expected here (Windows trip: none).

All uploads are client-to-service. No MoonClip server.

## 0. Estado (2026-09-27)

- **Google Drive: implementado.** OAuth loopback (puerto efímero, PKCE S256 +
  `state`, 3 min de timeout) contra el cliente Desktop del proyecto
  `moonclip-drive` (scope `drive.file` + `userinfo.email`); tokens como blob
  JSON en el keyring con refresh automático; carpeta raíz `MoonClip` y espejo
  `MoonClip/<juego>/` con mapeo cacheado en `drive_folders` (migración 014).
  Subida resumable por chunks de 8 MiB con progreso, reanudación exacta desde
  el `Range` del 308 y reintento de transporte; conflicto remoto → `_2`.
  **Privado por defecto**; el toggle "hacer público" crea el permiso
  anyone-reader y devuelve `webViewLink` para copiar. Bajada en streaming a la
  carpeta local del juego (thumbnail + probe + fila + `clip-saved`); conflicto
  local → `_2`. UI: Ajustes → Cuentas (conectar/desconectar) + botón
  compartir por clip + explorador Drive en la galería.
- **Credenciales**: `social.json` en el app-data dir (fuera de git, 0600) con
  override por env (`MOONCLIP_GOOGLE_DRIVE_CLIENT_ID`/`_SECRET`); plantilla
  `social.example.json`. El keyring ya no se expone por IPC.
- **Clips cloud (2026-09-27)**: tras subir, el checkbox opcional "borrar el
  video local" deja el clip solo en Drive (thumbnail local conservado, fila
  `cloud=1`, `drive_file_id`). El preview/trim usan `ensure_local` (descarga a
  la caché de la app con progreso, y el panel de trim la borra al cerrarse);
  el editor pesado descarga sí o sí al directorio de sesión (se limpia al
  cerrar) y el export reutiliza esa copia. Re-subir un clip cloud reutiliza el
  archivo remoto (sin duplicados). Borrar un clip cloud manda el remoto a la
  papelera de Drive (confirmación en dos pasos); purga y cuota ignoran filas
  cloud. Migración 015.
- **Subidas consistentes (2026-09-27)**: el `drive_file_id`/link se guarda en
  TODA subida (no solo al borrar el local); re-subir reutiliza el remoto
  (verificando que siga vivo; si está en papelera/404 se limpia y se sube de
  nuevo) y "Reemplazar en Drive" manda el viejo a la papelera. El diálogo de
  compartir tiene estado pendiente/subido (copiar link, hacer público, abrir
  en Drive, borrar local ahora, reemplazar).
- **Navegación estilo Medal (2026-09-27)**: galería en grid de tarjetas 16:9,
  sidebar con Todos/Favoritos/Subidos/En Drive/juegos y toolbar con búsqueda y
  orden. Al hacer clic se abre UN solo panel (el de recorte, agrandado) con
  reproductor, metadatos, renombrar inline, acciones (compartir, editor,
  favorito, revelar, borrar con confirmación cloud), navegación ←/→ y Esc;
  cloud descarga on demand con % y la copia temporal se borra al cerrar. El
  loop es opt-in (por defecto se detiene al final).
- **Detección**: además del blocklist de gestores, se rechazan títulos que
  terminan en `" - <app conocida>"` (Brave, Chrome, Firefox, Dolphin,
  Discord, VS Code, KWrite, Steam…) porque KRunner reporta `app_id` vacío
  para casi todo; los estados de juego (`- 1.4.4.9`) siguen matcheando.
- **YouTube (2026-09-27, código listo)**: mismo OAuth loopback de Google que
  Drive (scope `youtube.upload`, token propio en el keyring; si no hay
  `google_youtube` en `social.json` se usa el cliente de Drive). Subida
  resumible `videos.insert` (chunks de 8 MiB, reanudación desde el `Range`
  del 308, progreso por `moonclip://publish-progress`), título del usuario +
  descripción fija `#MoonClip #moonclip`, categoría Gaming, privacidad
  Privado/No listado/Público (default privado) y checkbox de cumplimiento de
  las normas de la comunidad. Solo subida: no navega ni descarga nada.
  **Bloqueo externo**: hasta pasar la auditoría de YouTube (y la verificación
  del scope sensible) todo video subido por API queda privado; la UI lo avisa
  (trámites en el log de PROGRESS).
- **Discord (2026-09-27)**: "conectar la cuenta" sin bot ni backend — OAuth
  `webhook.incoming` con Public Client + PKCE S256 (sin client secret) sobre
  el loopback fijo `127.0.0.1:38471`; el usuario elige servidor y canal en el
  navegador y el webhook que devuelve Discord (id/token/url) se guarda en el
  keyring, con el grant OAuth revocado al momento. Subida multipart
  `?wait=true` por streaming con progreso (`moonclip://publish-progress`,
  fases compress/upload), `payload_json` con título + `#MoonClip #moonclip` y
  menciones desactivadas; 413/429/401-404 con mensajes accionables. Si el clip
  supera el límite del servidor (ajuste `discord_max_mb`: 10/25/50/100, default
  10) se ofrece compresión agresiva a 720p (CRF 30 → bitrate calculado si aún
  no cabe, AAC 96k, una pista) a un temporal que se borra siempre.
  Desconectar borra el webhook del canal. La app de MoonClip va embebida como
  client id público (override en `social.json`/`MOONCLIP_DISCORD_CLIENT_ID`).
- Pendiente: TikTok (Worker broker para el `client_secret`) y X (navegador +
  portapapeles de archivo).

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
| Discord | Essential | OAuth `webhook.incoming` (Public Client + PKCE, no bot, no backend): the user picks a server + channel and the webhook lands in the keyring. `POST ?wait=true` multipart with streaming progress. Over-limit clips compress to a temporary 720p copy (optional; never a Drive-link fallback). Zero API cost. |
| YouTube | Essential | Data API v3 `videos.insert` over the same Google OAuth (implemented). Privacy selector: Private/Unlisted/Public. Fixed description `#MoonClip #moonclip`; default quota 100 `videos.insert`/day per project. Without the compliance audit, API uploads are locked private. |
| TikTok | Essential | Content Posting API (Direct Post / Inbox Draft). Requires TikTok Developers app + audit. Upload as draft so user adds music. Fallback: open TikTok Studio Web with file ready. |
| Twitter/X | Essential | **No paid API.** Copy file to OS clipboard + open `https://twitter.com/compose/tweet?text=...` (or `twitter.com/intent/tweet?url=<drive>&text=...`). User presses Ctrl+V; video uploads natively. If using Drive link, ensure OpenGraph `twitter:card=player` on viewer page (future web viewer). |
| Instagram/Facebook | Optional | Meta Graph API requires Business/Creator + audit; desktop Reels restricted. Defer past MVP. |

## 3. Acceptance (Phase 6)

- [x] Drive upload shows live %, finishes with public `webViewLink` copied + notification.
- [ ] YouTube upload shows live %, finishes with the video link (public visibility waits for the project audit).
- [ ] Discord: connect flow captures the webhook; the upload sends the file
      (or the compressed copy) with live progress (code + mock tests green;
      owner live pass pending).
- [ ] Twitter flow opens intent with clipboard ready.
