/**
 * MoonClip OAuth broker (TikTok).
 *
 * Holds the TikTok client_secret so the desktop app never ships it. The app
 * sends the PKCE code + verifier and receives TikTok's token JSON back.
 *
 * Endpoints:
 *   POST /tiktok/exchange  { code, code_verifier, redirect_uri }
 *   POST /tiktok/refresh   { refresh_token }
 *   GET  /health
 *
 * No CORS headers on purpose: only the MoonClip desktop app calls this.
 */

export interface Env {
  TIKTOK_CLIENT_KEY: string;
  TIKTOK_CLIENT_SECRET: string;
}

const TOKEN_URL = "https://open.tiktokapis.com/v2/oauth/token/";
const JSON_HEADERS = { "Content-Type": "application/json; charset=utf-8" };
const MAX_BODY = 4096;

function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), { status, headers: JSON_HEADERS });
}

async function readBody(request: Request): Promise<Record<string, string> | null> {
  const text = await request.text();
  if (text.length > MAX_BODY) return null;
  try {
    const parsed: unknown = JSON.parse(text);
    if (typeof parsed !== "object" || parsed === null) return null;
    return parsed as Record<string, string>;
  } catch {
    return null;
  }
}

/** TikTok desktop redirect URIs: loopback host + port + `/callback/`. */
function validRedirect(uri: string): boolean {
  return /^http:\/\/127\.0\.0\.1:\d{1,5}\/callback\/$/.test(uri);
}

async function postToken(form: Record<string, string>): Promise<Response> {
  const response = await fetch(TOKEN_URL, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams(form).toString(),
  });
  return new Response(await response.text(), {
    status: response.status,
    headers: JSON_HEADERS,
  });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname } = new URL(request.url);
    if (pathname === "/health") return json({ ok: true });
    if (request.method !== "POST") return json({ error: "method_not_allowed" }, 405);

    if (pathname === "/tiktok/exchange") {
      const body = await readBody(request);
      if (!body) return json({ error: "invalid_body" }, 400);
      const { code, code_verifier, redirect_uri } = body;
      if (!code || !code_verifier || !redirect_uri || !validRedirect(redirect_uri)) {
        return json({ error: "invalid_request" }, 400);
      }
      return postToken({
        client_key: env.TIKTOK_CLIENT_KEY,
        client_secret: env.TIKTOK_CLIENT_SECRET,
        code,
        grant_type: "authorization_code",
        redirect_uri,
        code_verifier,
      });
    }

    if (pathname === "/tiktok/refresh") {
      const body = await readBody(request);
      const refreshToken = body?.refresh_token;
      if (!refreshToken) return json({ error: "invalid_request" }, 400);
      return postToken({
        client_key: env.TIKTOK_CLIENT_KEY,
        client_secret: env.TIKTOK_CLIENT_SECRET,
        grant_type: "refresh_token",
        refresh_token: refreshToken,
      });
    }

    return json({ error: "not_found" }, 404);
  },
};
