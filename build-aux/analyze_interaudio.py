#!/usr/bin/env python3
"""Inter-audio sync check for MoonClip clips (SPEC §14).

Cross-correlates the clip's audio stems to verify they are aligned with each
other, and checks that the Master track equals the clamped sample sum of the
solo stems.

Usage:
  python build-aux/analyze_interaudio.py <clip.mp4> [--ffmpeg PATH]
                                         [--window 5] [--offset 2]
  python build-aux/analyze_interaudio.py --selftest

Track layout (1-based ffmpeg indexes): 0 = Master Mix, 1 = Game/Desktop,
2 = Microphone.

Method: decode a mono 8 kHz window per track, FFT cross-correlation with
parabolic peak interpolation (~0.02 ms resolution). Pure Python (no numpy).

Sign convention: `lag = +X ms` means the FIRST track of the pair leads the
SECOND one by X ms (the second needs +X ms of delay to align).

Thresholds: inter-audio |lag| <= 20 ms (target < 10 ms), Master vs clamped
sum <= 1 LSB. Exit code 0 when everything passes.
"""

import argparse
import cmath
import math
import os
import struct
import subprocess
import sys

RATE = 8000
INTER_AUDIO_MAX_MS = 20.0


def default_ffmpeg() -> str:
    env = os.environ.get("MOONCLIP_FFMPEG")
    if env and os.path.exists(env):
        return env
    here = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    candidate = os.path.join(
        here, "src-tauri", "binaries", "x86_64-pc-windows-msvc",
        "ffmpeg-x86_64-pc-windows-msvc.exe",
    )
    if os.path.exists(candidate):
        return candidate
    return "ffmpeg"


def decode(ffmpeg, clip, track, seconds, offset, rate=RATE, channels=1):
    cmd = [
        ffmpeg, "-v", "error",
        "-ss", str(offset), "-t", str(seconds), "-i", clip,
        "-map", f"0:a:{track}", "-ac", str(channels), "-ar", str(rate),
        "-f", "s16le", "-",
    ]
    out = subprocess.run(cmd, capture_output=True)
    if out.returncode != 0:
        raise SystemExit(
            f"ffmpeg failed for track {track}: "
            f"{out.stderr.decode(errors='replace')[:400]}"
        )
    raw = out.stdout
    count = len(raw) // 2
    return list(struct.unpack(f"<{count}h", raw[: count * 2]))


def fft(a):
    n = len(a)
    if n == 1:
        return a
    even = fft(a[0::2])
    odd = fft(a[1::2])
    out = [0j] * n
    for k in range(n // 2):
        t = cmath.exp(-2j * math.pi * k / n) * odd[k]
        out[k] = even[k] + t
        out[k + n // 2] = even[k] - t
    return out


def ifft(a):
    n = len(a)
    conj = [x.conjugate() for x in a]
    y = fft(conj)
    return [x.conjugate() / n for x in y]


def energy(samples):
    return sum(v * v for v in samples)


def xcorr_lag_ms(x, y, rate=RATE):
    """Lag of x relative to y in ms (+ = x leads y). None when too silent."""
    if energy(x) == 0 or energy(y) == 0:
        return None
    n = 1
    while n < len(x) + len(y):
        n <<= 1
    fx = fft([complex(v, 0) for v in x] + [0j] * (n - len(x)))
    fy = fft([complex(v, 0) for v in y] + [0j] * (n - len(y)))
    corr = ifft([fx[i] * fy[i].conjugate() for i in range(n)])
    mags = [abs(v) for v in corr]
    peak = max(range(n), key=lambda i: mags[i])
    lag = peak if peak < n // 2 else peak - n
    # Parabolic interpolation around the peak for sub-sample accuracy.
    left = mags[(peak - 1) % n]
    mid = mags[peak]
    right = mags[(peak + 1) % n]
    denom = left - 2 * mid + right
    delta = 0.0 if denom == 0 else 0.5 * (left - right) / denom
    # R_xy[k] = sum x[i] y[i-k] peaks at k = -(lead of x over y); negate so a
    # positive value means "x leads y".
    return -(lag + delta) / rate * 1000.0


def master_diff(ffmpeg, clip, seconds, offset):
    """Max |Master - clamp(Game + Mic)| in LSB at 48 kHz stereo."""
    game = decode(ffmpeg, clip, 1, seconds, offset, rate=48000, channels=2)
    mic = decode(ffmpeg, clip, 2, seconds, offset, rate=48000, channels=2)
    master = decode(ffmpeg, clip, 0, seconds, offset, rate=48000, channels=2)
    n = min(len(game), len(mic), len(master))
    worst = 0
    for i in range(0, n, 7):
        s = game[i] + mic[i]
        s = -32768 if s < -32768 else (32767 if s > 32767 else s)
        d = abs(s - master[i])
        if d > worst:
            worst = d
    return worst


def selftest():
    # Synthetic 1 kHz tone, y delayed by 3.25 ms (26 samples at 8 kHz).
    samples = 4000
    tone = [int(12000 * math.sin(2 * math.pi * 1000 * i / RATE)) for i in range(samples)]
    delay = 26
    x = tone
    y = [0] * delay + tone[: samples - delay]
    lag = xcorr_lag_ms(x, y)
    expected = delay / RATE * 1000.0
    print(f"selftest: lag={lag:.3f} ms expected={expected:.3f} ms")
    ok = lag is not None and abs(lag - expected) < 0.2
    print("selftest:", "OK" if ok else "FAIL")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser(description="MoonClip inter-audio sync check")
    ap.add_argument("clip", nargs="?")
    ap.add_argument("--ffmpeg", default=default_ffmpeg())
    ap.add_argument("--window", type=float, default=5.0)
    ap.add_argument("--offset", type=float, default=2.0)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    if not args.clip:
        ap.error("clip path is required (or use --selftest)")
    if not os.path.exists(args.clip):
        raise SystemExit(f"clip not found: {args.clip}")

    print(f"[{args.clip}] window={args.window}s offset={args.offset}s")
    try:
        master = decode(args.ffmpeg, args.clip, 0, args.window, args.offset)
        game = decode(args.ffmpeg, args.clip, 1, args.window, args.offset)
        mic = decode(args.ffmpeg, args.clip, 2, args.window, args.offset)
    except SystemExit as e:
        print(str(e))
        return 1

    pairs = [
        ("Game", game, "Mic", mic),
        ("Master", master, "Game", game),
        ("Master", master, "Mic", mic),
    ]
    failures = 0
    for a_name, a, b_name, b in pairs:
        lag = xcorr_lag_ms(a, b)
        if lag is None:
            print(f"  {a_name:6s} vs {b_name:6s}: SKIP (silent window)")
            continue
        ok = abs(lag) <= INTER_AUDIO_MAX_MS
        failures += 0 if ok else 1
        print(
            f"  {a_name:6s} vs {b_name:6s}: lag {lag:+7.2f} ms "
            f"(limit {INTER_AUDIO_MAX_MS:.0f} ms)  {'OK' if ok else 'FAIL'}"
        )

    diff = master_diff(args.ffmpeg, args.clip, min(args.window, 3.0), args.offset)
    sum_ok = diff <= 1
    failures += 0 if sum_ok else 1
    print(f"  Master vs clamp(Game+Mic): max |diff| = {diff} LSB (target <= 1)  "
          f"{'OK' if sum_ok else 'FAIL'}")

    print("RESULT:", "OK" if failures == 0 else f"{failures} FAIL")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
