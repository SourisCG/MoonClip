#!/usr/bin/env python3
"""E2E: trigger a replay-buffer save over obs-websocket (v5) and verify the
resulting clip has 1 video + 3 audio tracks. Also proves the patched mux
binary (moonclip-mux) can be spawned at save time.

Usage:
  python3 build-aux/linux/tests/save_replay.py [--timeout 60]

Requires the `websockets` package (pip install websockets) and the app running
with the replay buffer active (MOONCLIP_E2E_AUTOSTART=1 pnpm tauri:dev).
Reads port/password and the clips dir straight from MoonClip's SQLite.
"""
import argparse
import asyncio
import base64
import hashlib
import json
import sqlite3
import subprocess
import sys
import time
from pathlib import Path

DB = Path.home() / ".local/share/dev.souriscg.moonclip/moonclip.db"
BUNDLE = Path(__file__).resolve().parents[3] / "src-tauri/binaries/x86_64-unknown-linux-gnu"
FFMPEG = BUNDLE / "ffmpeg-x86_64-unknown-linux-gnu"


def read_settings():
    con = sqlite3.connect(DB)
    try:
        rows = dict(con.execute("SELECT key, value FROM settings").fetchall())
    finally:
        con.close()
    return rows


async def request(ws, request_type, request_id, **kwargs):
    await ws.send(
        json.dumps(
            {
                "op": 6,
                "d": {"requestType": request_type, "requestId": request_id, **kwargs},
            }
        )
    )
    while True:
        msg = json.loads(await ws.recv())
        if msg.get("op") == 7 and msg["d"].get("requestId") == request_id:
            d = msg["d"]
            if not d.get("requestStatus", {}).get("result"):
                raise RuntimeError(f"{request_type} failed: {d}")
            return d.get("responseData", {})


async def save(timeout):
    settings = read_settings()
    port = int(settings.get("obs_ws_port", "4456"))
    password = settings.get("obs_ws_password", "")
    import websockets  # noqa: PLC0415

    async with websockets.connect(f"ws://127.0.0.1:{port}") as ws:
        hello = json.loads(await ws.recv())["d"]
        identify = {"rpcVersion": 1, "eventSubscriptions": 0}
        auth = hello.get("authentication")
        if auth:
            secret = base64.b64encode(
                hashlib.sha256((password + auth["salt"]).encode()).digest()
            ).decode()
            identify["authentication"] = base64.b64encode(
                hashlib.sha256((secret + auth["challenge"]).encode()).digest()
            ).decode()
        await ws.send(json.dumps({"op": 1, "d": identify}))
        identified = json.loads(await ws.recv())
        assert identified.get("op") == 2, identified

        status = await request(ws, "GetReplayBufferStatus", "s1")
        if not status.get("outputActive"):
            raise RuntimeError("replay buffer is not active (start the buffer first)")

        previous = (await request(ws, "GetLastReplayBufferReplay", "s2")).get("savedReplayPath", "")
        print(f"previous replay: {previous or '(none)'}")

        await request(ws, "SaveReplayBuffer", "s3")
        deadline = time.monotonic() + timeout
        path = ""
        while time.monotonic() < deadline:
            path = (await request(ws, "GetLastReplayBufferReplay", "s4")).get("savedReplayPath", "")
            if path and path != previous:
                break
            await asyncio.sleep(0.5)
        if not path or path == previous:
            raise RuntimeError(f"no new replay saved within {timeout}s (last: {path!r})")
        return Path(path)


def probe(clip):
    out = subprocess.run(
        [str(FFMPEG), "-hide_banner", "-i", str(clip)],
        capture_output=True,
        text=True,
    )
    kinds = []
    for line in out.stderr.splitlines():
        if "Stream #" not in line:
            continue
        if "Video:" in line:
            kinds.append("video")
        elif "Audio:" in line:
            kinds.append("audio")
    return kinds


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--timeout", type=float, default=60.0)
    args = ap.parse_args()

    clip = asyncio.run(save(args.timeout))
    if not clip.exists() or clip.stat().st_size < 1024 * 1024:
        print(f"FAIL clip missing or too small: {clip}")
        return 1
    kinds = probe(clip)
    video = kinds.count("video")
    audio = kinds.count("audio")
    print(f"OK  clip {clip.name} ({clip.stat().st_size // (1024 * 1024)} MB, "
          f"video={video} audio={audio})")
    if video != 1 or audio != 3:
        print("FAIL expected 1 video + 3 audio tracks")
        return 1
    print("PASS save-replay")
    return 0


if __name__ == "__main__":
    sys.exit(main())
