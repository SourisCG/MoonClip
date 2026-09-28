# moonclip-oauth

Tiny Cloudflare Worker that brokers TikTok OAuth so the `client_secret` never
ships inside the MoonClip desktop app (GPL, public repo).

The desktop app still uses the TikTok desktop flow end to end: loopback
redirect (`http://127.0.0.1:<port>/callback/`) + PKCE S256. The Worker only
adds the secret at exchange/refresh time.

## Endpoints

| Method | Path | Body | Returns |
|---|---|---|---|
| POST | `/tiktok/exchange` | `{ "code", "code_verifier", "redirect_uri" }` | TikTok token JSON (access + refresh token) |
| POST | `/tiktok/refresh` | `{ "refresh_token" }` | TikTok token JSON (rotated refresh token) |
| GET | `/health` | — | `{ "ok": true }` |

`redirect_uri` must match `http://127.0.0.1:<port>/callback/`. No CORS headers:
only the desktop app calls this.

## Deploy

```bash
pnpm install
pnpm exec wrangler login
pnpm exec wrangler secret put TIKTOK_CLIENT_KEY
pnpm exec wrangler secret put TIKTOK_CLIENT_SECRET
pnpm exec wrangler deploy
```

The deploy prints the Worker URL
(`https://moonclip-oauth.<your-subdomain>.workers.dev`). Put it in the app's
`social.json` as `tiktok_worker_url` (or the `MOONCLIP_TIKTOK_WORKER_URL`
environment variable). The app needs no TikTok client secret of its own in
this mode.

## Local development

```bash
pnpm exec wrangler dev                 # http://127.0.0.1:8787
curl http://127.0.0.1:8787/health
```

With `wrangler dev`, create a `.dev.vars` file (gitignored) for the secrets:

```
TIKTOK_CLIENT_KEY="..."
TIKTOK_CLIENT_SECRET="..."
```
