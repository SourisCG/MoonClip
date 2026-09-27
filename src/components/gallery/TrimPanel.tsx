import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Loader2, Pause, Play, Scissors, Wand2, X } from "lucide-react";
import type { ClipMetadata } from "../../types";

/** Payload of `moonclip://edit-progress` (Rust `EditProgress`). */
interface EditProgress {
  op: string;
  clipId: string;
  percent: number;
  done: boolean;
}

function fmt(ms: number) {
  const total = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}.${String(
    Math.max(0, Math.round(ms / 100)) % 10,
  )}`;
}

/**
 * Gallery quick trim: light panel (no editor chunk, no extra libraries).
 * Lossless copy by default; precise re-encode on demand. Saves a NEW clip.
 */
export function TrimPanel({
  clip,
  onClose,
  onSaved,
  onAdvancedEdit,
}: {
  clip: ClipMetadata;
  onClose: () => void;
  onSaved: () => void;
  onAdvancedEdit?: (clip: ClipMetadata) => void;
}) {
  const { t } = useTranslation();
  const videoRef = useRef<HTMLVideoElement>(null);
  const barRef = useRef<HTMLDivElement>(null);
  const [url, setUrl] = useState<string | null>(null);
  const [duration, setDuration] = useState(Math.max(100, clip.duration_ms));
  const [start, setStart] = useState(0);
  const [end, setEnd] = useState(Math.max(100, clip.duration_ms));
  const [pos, setPos] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [precise, setPrecise] = useState(false);
  const [saving, setSaving] = useState(false);
  const [percent, setPercent] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [drag, setDrag] = useState<"start" | "end" | "seek" | null>(null);

  // Closing the panel releases the on-demand copy of a cloud clip (local
  // clips have no cache, so this is a no-op for them).
  const close = useCallback(() => {
    void invoke("cloud_cache_cleanup", { clipId: clip.id }).catch(() => {});
    onClose();
  }, [clip.id, onClose]);

  useEffect(() => {
    let cancelled = false;
    // Loopback HTTP URL: `asset://` cannot play media on WebKitGTK.
    invoke<string>("media_url", { clipId: clip.id })
      .then((u) => {
        if (!cancelled) setUrl(u);
      })
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [clip.id]);

  // Tear the media pipeline down on close: WebKitGTK can keep playing audio
  // from a removed <video> unless the source is cleared explicitly. The
  // element is captured from the `url` effect (by unmount time the ref is
  // already detached).
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    return () => {
      try {
        v.pause();
        v.removeAttribute("src");
        v.load();
      } catch {
        /* element already gone */
      }
    };
  }, [url]);

  // Leaving the app (alt-tab / minimize) must not keep the preview sounding.
  useEffect(() => {
    const pause = () => videoRef.current?.pause();
    const onVisibility = () => {
      if (document.hidden) pause();
    };
    window.addEventListener("blur", pause);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("blur", pause);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  // Trim progress for THIS clip (the backend emits per handled clip).
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void listen<EditProgress>("moonclip://edit-progress", (e) => {
      if (e.payload.op === "trim" && e.payload.clipId === clip.id) {
        setPercent(e.payload.percent);
      }
    }).then((u) => {
      unlisten = u;
    });
    return () => unlisten?.();
  }, [clip.id]);

  // Real media duration wins over the stored one (legacy rows may be off).
  const onLoaded = () => {
    const v = videoRef.current;
    if (v && Number.isFinite(v.duration) && v.duration > 0) {
      const ms = Math.round(v.duration * 1000);
      setDuration(ms);
      setEnd((e) => Math.min(e, ms));
    }
  };

  const seek = useCallback((ms: number) => {
    const v = videoRef.current;
    if (v) v.currentTime = Math.max(0, ms) / 1000;
    setPos(ms);
  }, []);

  const togglePlay = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    if (v.paused) {
      // Never play outside the selection: jump to In first.
      const ms = v.currentTime * 1000;
      if (ms < start || ms >= end - 20) seek(start);
      void v.play();
    } else {
      v.pause();
    }
  }, [end, seek, start]);

  const onTimeUpdate = () => {
    const v = videoRef.current;
    if (!v) return;
    const ms = v.currentTime * 1000;
    // Preview is bounded to [start, end]: the audio never plays outside the
    // selection (looping while playing, parked at the edge when paused).
    if (ms < start - 30) {
      v.currentTime = start / 1000;
      setPos(start);
      return;
    }
    if (ms >= end) {
      if (!v.paused) {
        v.currentTime = start / 1000;
      } else {
        v.currentTime = end / 1000;
      }
      setPos(end);
      return;
    }
    setPos(ms);
  };

  const msFromClientX = useCallback(
    (clientX: number) => {
      const bar = barRef.current;
      if (!bar) return 0;
      const r = bar.getBoundingClientRect();
      const x = Math.min(Math.max(clientX - r.left, 0), r.width);
      return (x / r.width) * duration;
    },
    [duration],
  );

  const onBarMove = (e: React.PointerEvent) => {
    if (!drag) return;
    const ms = msFromClientX(e.clientX);
    if (drag === "start") {
      const next = Math.min(ms, end - 200);
      setStart(next);
      seek(next);
    } else if (drag === "end") {
      const next = Math.max(ms, start + 200);
      setEnd(next);
    } else {
      seek(Math.min(Math.max(ms, start), end));
    }
  };

  const onBarDown = (e: React.PointerEvent) => {
    barRef.current?.setPointerCapture(e.pointerId);
    setDrag("seek");
    seek(Math.min(Math.max(msFromClientX(e.clientX), start), end));
  };

  const save = async () => {
    videoRef.current?.pause();
    setSaving(true);
    setError(null);
    setPercent(0);
    try {
      await invoke("trim_clip", {
        clipId: clip.id,
        startMs: Math.round(start),
        endMs: Math.round(end),
        precise,
      });
      onSaved();
      close();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const handle =
    "absolute top-0 h-full w-3 -translate-x-1/2 cursor-ew-resize rounded-full border border-cyan-300/60 bg-cyan-400/70";
  const pct = (ms: number) => `${(ms / Math.max(1, duration)) * 100}%`;

  return (
    <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/60 p-4">
      <div className="w-full max-w-3xl rounded-2xl border border-white/10 bg-[#0b0f19] p-4 shadow-2xl">
        <div className="mb-3 flex items-center gap-2">
          <Scissors size={16} className="text-cyan-300" />
          <h3 className="text-sm font-semibold text-slate-200">{t("trim.title")}</h3>
          <span className="min-w-0 flex-1 truncate font-mono text-xs text-slate-500">
            {clip.file_name}
          </span>
          <button
            onClick={close}
            disabled={saving}
            className="rounded-lg p-1.5 text-slate-500 transition hover:bg-white/10 hover:text-slate-200 disabled:opacity-50"
          >
            <X size={16} />
          </button>
        </div>

        <div className="overflow-hidden rounded-xl bg-black/60">
          {url ? (
            <video
              ref={videoRef}
              src={url}
              onLoadedMetadata={onLoaded}
              onTimeUpdate={onTimeUpdate}
              onPlay={() => setPlaying(true)}
              onPause={() => setPlaying(false)}
              onClick={togglePlay}
              onError={() => {
                const code = videoRef.current?.error?.code;
                setError(`${t("trim.play_error")} (${code ?? "?"})`);
              }}
              className="mx-auto max-h-[46vh] w-full cursor-pointer object-contain"
            />
          ) : (
            <div className="flex h-48 items-center justify-center text-xs text-slate-500">
              {t("common.loading")}
            </div>
          )}
        </div>

        <div className="mt-3 flex items-center gap-3">
          <button
            onClick={togglePlay}
            className="rounded-lg border border-white/10 bg-white/5 p-2 text-slate-200 transition hover:bg-white/10"
            title={playing ? t("trim.pause") : t("trim.play")}
          >
            {playing ? <Pause size={14} /> : <Play size={14} />}
          </button>
          <span className="w-24 shrink-0 font-mono text-xs text-slate-400">
            {fmt(pos)} / {fmt(duration)}
          </span>

          <div
            ref={barRef}
            onPointerDown={onBarDown}
            onPointerMove={onBarMove}
            onPointerUp={() => setDrag(null)}
            onPointerCancel={() => setDrag(null)}
            className="relative h-8 flex-1 cursor-pointer touch-none select-none rounded-lg border border-white/10 bg-white/5"
          >
            <div
              className="absolute inset-y-0 bg-cyan-400/20"
              style={{ left: pct(start), width: pct(end - start) }}
            />
            <div
              className="absolute inset-y-0 w-0.5 bg-cyan-300"
              style={{ left: pct(pos) }}
            />
            <div
              role="slider"
              aria-valuenow={Math.round(start)}
              className={handle}
              style={{ left: pct(start) }}
              onPointerDown={(e) => {
                e.stopPropagation();
                setDrag("start");
              }}
            />
            <div
              role="slider"
              aria-valuenow={Math.round(end)}
              className={handle}
              style={{ left: pct(end) }}
              onPointerDown={(e) => {
                e.stopPropagation();
                setDrag("end");
              }}
            />
          </div>
        </div>

        <div className="mt-3 flex flex-wrap items-center gap-3">
          <label className="flex items-center gap-2 text-xs text-slate-300">
            <input
              type="checkbox"
              checked={precise}
              onChange={(e) => setPrecise(e.target.checked)}
              disabled={saving}
              className="accent-cyan-400"
            />
            {t("trim.precise")}
          </label>
          <span className="text-[11px] text-slate-500">{t("trim.hint")}</span>
          <span className="ml-auto font-mono text-xs text-slate-400">
            {fmt(start)} – {fmt(end)} ({((end - start) / 1000).toFixed(1)}s)
          </span>
        </div>

        {saving && (
          <div className="mt-3">
            <div className="h-1.5 overflow-hidden rounded-full bg-white/10">
              <div
                className="h-full bg-cyan-400 transition-[width] duration-200"
                style={{ width: `${Math.min(100, percent)}%` }}
              />
            </div>
            <p className="mt-1 text-[11px] text-slate-400">{t("trim.saving")}</p>
          </div>
        )}
        {error && <p className="mt-2 break-words font-mono text-xs text-red-400">{error}</p>}

        <div className="mt-4 flex justify-end gap-2">
          {onAdvancedEdit && (
            <button
              onClick={() => {
                close();
                onAdvancedEdit(clip);
              }}
              disabled={saving}
              className="mr-auto inline-flex items-center gap-1.5 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-3 py-1.5 text-sm text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
            >
              <Wand2 size={14} /> {t("editor.open")}
            </button>
          )}
          <button
            onClick={close}
            disabled={saving}
            className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-sm text-slate-200 transition hover:bg-white/10 disabled:opacity-50"
          >
            {t("trim.cancel")}
          </button>
          <button
            onClick={() => void save()}
            disabled={saving || end - start < 100}
            className="inline-flex items-center gap-1.5 rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-3 py-1.5 text-sm text-cyan-200 transition hover:bg-cyan-500/20 disabled:cursor-wait disabled:opacity-50"
          >
            {saving ? <Loader2 size={14} className="animate-spin" /> : <Scissors size={14} />}
            {saving ? t("trim.saving_short") : t("trim.save")}
          </button>
        </div>
      </div>
    </div>
  );
}
