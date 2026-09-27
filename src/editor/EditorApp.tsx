import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import WaveSurfer from "wavesurfer.js";
import {
  ArrowLeft,
  Copy,
  Download,
  Loader2,
  Pause,
  Play,
  Redo2,
  Scissors,
  Trash2,
  Undo2,
  Volume2,
  VolumeX,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { TimelineView } from "./TimelineView";
import { ExportDialog } from "./ExportDialog";
import { canRedo, canUndo, redo, undo, useEditorStore } from "./store";
import { projectDurationMs, type EditorOpenResult } from "./types";

/** Heavy editor (E2): lazy chunk, maximized in-app view, fully torn down on
 *  close (session temps, waveforms, playback and project state). */
export default function EditorApp({
  clipId,
  onExit,
  onClipSaved,
}: {
  clipId: string;
  onExit: () => void;
  onClipSaved: () => void;
}) {
  const { t } = useTranslation();
  const [session, setSession] = useState<EditorOpenResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [visibleMs, setVisibleMs] = useState(8000);
  const [showExport, setShowExport] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [toast, setToast] = useState<string | null>(null);

  const videoRef = useRef<HTMLVideoElement | null>(null);
  const laneRefs = useRef<(HTMLDivElement | null)[]>([]);
  const wsRef = useRef<WaveSurfer[]>([]);
  const saveTimer = useRef<number | undefined>(undefined);
  const openingRef = useRef(false);
  const sessionRef = useRef<EditorOpenResult | null>(null);
  const closedRef = useRef(false);

  const project = useEditorStore((s) => s.project);
  const selectedId = useEditorStore((s) => s.selectedSegmentId);
  const playing = useEditorStore((s) => s.playing);
  const total = project ? projectDurationMs(project) : 0;
  const selected = project?.segments.find((s) => s.id === selectedId) ?? project?.segments[0];

  // ---- Session lifecycle -------------------------------------------------
  useEffect(() => {
    if (openingRef.current) return;
    openingRef.current = true;
    invoke<EditorOpenResult>("editor_open", { clipId })
      .then((s) => {
        sessionRef.current = s;
        useEditorStore.getState().setProject(s.project);
        useEditorStore.temporal.getState().clear();
        setSession(s);
        setVisibleMs(Math.max(6000, Math.min(30_000, s.project.segments[0]?.outMs ?? 8000) + 2000));
      })
      .catch((e) => setError(String(e)));
    return () => {
      // Full teardown, even if the view is yanked out: WebKit keeps media
      // alive unless src is cleared, and the session dir must not leak.
      wsRef.current.forEach((w) => w.destroy());
      wsRef.current = [];
      const v = videoRef.current;
      if (v) {
        v.pause();
        v.removeAttribute("src");
        v.load();
      }
      if (!closedRef.current && sessionRef.current) {
        closedRef.current = true;
        void invoke("editor_close", { sessionId: sessionRef.current.sessionId }).catch(() => {});
      }
    };
  }, [clipId]);

  // ---- Playback ----------------------------------------------------------
  const syncAudios = useCallback((ms: number) => {
    wsRef.current.forEach((w) => {
      try {
        w.setTime(ms / 1000);
      } catch {
        /* media not ready yet */
      }
    });
  }, []);

  const seek = useCallback(
    (ms: number) => {
      const v = videoRef.current;
      const target = Math.max(0, Math.min(ms, Math.max(0, total - 30)));
      if (v) v.currentTime = target / 1000;
      syncAudios(target);
      useEditorStore.getState().setPlayhead(target);
    },
    [syncAudios, total],
  );

  const pause = useCallback(() => {
    videoRef.current?.pause();
    wsRef.current.forEach((w) => w.pause());
    useEditorStore.getState().setPlaying(false);
  }, []);

  const exit = useCallback(async () => {
    pause();
    if (sessionRef.current && !closedRef.current) {
      closedRef.current = true;
      await invoke("editor_close", { sessionId: sessionRef.current.sessionId }).catch(() => {});
    }
    useEditorStore.getState().resetEditor();
    useEditorStore.temporal.getState().clear();
    onExit();
  }, [pause, onExit]);

  const play = useCallback(() => {
    const v = videoRef.current;
    if (!v) return;
    const ms = useEditorStore.getState().playheadMs;
    if (ms >= total - 30) seek(0);
    syncAudios(useEditorStore.getState().playheadMs);
    void v.play();
    wsRef.current.forEach((w) => void w.play());
    useEditorStore.getState().setPlaying(true);
  }, [seek, syncAudios, total]);

  const toggle = useCallback(() => {
    if (useEditorStore.getState().playing) pause();
    else play();
  }, [pause, play]);

  // Playhead sync + drift correction while playing.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    let lastDrift = 0;
    const tick = () => {
      const v = videoRef.current;
      if (!v) return;
      const ms = v.currentTime * 1000;
      useEditorStore.getState().setPlayhead(ms);
      const now = performance.now();
      if (now - lastDrift > 500) {
        lastDrift = now;
        wsRef.current.forEach((w) => {
          try {
            if (Math.abs(w.getCurrentTime() * 1000 - ms) > 250) w.setTime(ms / 1000);
          } catch {
            /* not ready */
          }
        });
      }
      if (ms >= total - 20) {
        pause();
        seek(0);
        return;
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, pause, seek, total]);

  // ---- Waveforms (one per capture track) ---------------------------------
  useEffect(() => {
    if (!session) return;
    const created: WaveSurfer[] = [];
    session.audioTracks.forEach((track, i) => {
      const el = laneRefs.current[i];
      if (!el) return;
      const ws = WaveSurfer.create({
        container: el,
        url: track.url,
        height: 32,
        waveColor: "#334155",
        progressColor: "#0891b2",
        cursorWidth: 0,
        barWidth: 1,
        barGap: 1,
        normalize: true,
        interact: false,
      });
      created.push(ws);
    });
    wsRef.current = created;
    return () => {
      created.forEach((w) => w.destroy());
      wsRef.current = [];
    };
  }, [session]);

  // Gains -> waveform volumes (preview caps at 100%; export uses the real gain).
  useEffect(() => {
    if (!session || !selected) return;
    const gains = [selected.gainMix, selected.gainGame, selected.gainMic];
    wsRef.current.forEach((w, i) => w.setVolume(Math.min(1, gains[i] ?? 1)));
  }, [session, selected]);

  // ---- Autosave ----------------------------------------------------------
  useEffect(() => {
    if (!session) return;
    const unsub = useEditorStore.subscribe((s, prev) => {
      if (!s.project || s.project === prev.project) return;
      setDirty(true);
      window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => {
        invoke("editor_save_project", { project: s.project })
          .then(() => setDirty(false))
          .catch(() => {});
      }, 800);
    });
    return () => {
      unsub();
      window.clearTimeout(saveTimer.current);
    };
  }, [session]);

  // ---- Hotkeys -----------------------------------------------------------
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable)) {
        return;
      }
      const mod = e.ctrlKey || e.metaKey;
      if (mod && e.key.toLowerCase() === "z" && !e.shiftKey) {
        e.preventDefault();
        undo();
      } else if (mod && (e.key.toLowerCase() === "y" || (e.shiftKey && e.key.toLowerCase() === "z"))) {
        e.preventDefault();
        redo();
      } else if (mod && (e.key.toLowerCase() === "k" || e.key.toLowerCase() === "s")) {
        e.preventDefault();
        useEditorStore.getState().splitAtPlayhead();
      } else if (mod && e.key.toLowerCase() === "d") {
        e.preventDefault();
        if (selectedId) useEditorStore.getState().duplicateSegment(selectedId);
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        if (selectedId) useEditorStore.getState().deleteSegment(selectedId);
      } else if (e.key === " ") {
        e.preventDefault();
        toggle();
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        seek(useEditorStore.getState().playheadMs - (e.shiftKey ? 1000 : 33));
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        seek(useEditorStore.getState().playheadMs + (e.shiftKey ? 1000 : 33));
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [seek, selectedId, toggle]);

  const exportDone = useCallback(() => {
    onClipSaved();
    setToast(t("editor.export_done"));
    setShowExport(false);
    window.setTimeout(() => setToast(null), 4000);
  }, [onClipSaved, t]);

  // ---- Render ------------------------------------------------------------
  if (error) {
    return (
      <div className="flex h-full items-center justify-center">
        <div className="max-w-md rounded-xl border border-red-500/30 bg-red-500/10 p-4 text-sm text-red-200">
          <p className="break-words font-mono text-xs">{error}</p>
          <button
            onClick={() => void exit()}
            className="mt-3 rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-slate-200"
          >
            {t("editor.back")}
          </button>
        </div>
      </div>
    );
  }
  if (!session || !project) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-slate-400">
        <Loader2 size={22} className="animate-spin" />
        <p className="text-sm">{t("editor.loading")}</p>
      </div>
    );
  }

  const iconBtn =
    "rounded-lg border border-white/10 bg-white/5 p-2 text-slate-300 transition hover:bg-white/10 disabled:opacity-40";
  type GainField = "gainMix" | "gainGame" | "gainMic";
  const gains: [string, GainField, string][] = [
    ["mix", "gainMix", t("editor.mix")],
    ["game", "gainGame", t("editor.game")],
    ["mic", "gainMic", t("editor.mic")],
  ];

  return (
    <div className="flex h-full min-h-0 flex-col bg-[#070a12]">
      {/* Header */}
      <div className="flex flex-wrap items-center gap-2 border-b border-white/10 px-3 py-2">
        <button onClick={() => void exit()} className={iconBtn} title={t("editor.back")}>
          <ArrowLeft size={15} />
        </button>
        <div className="min-w-0">
          <p className="truncate text-sm font-semibold text-slate-100">{project.name}</p>
          <p className="truncate text-[11px] text-slate-500">
            {session.clip.gameTitle} · {session.clip.width}×{session.clip.height}
            {session.usingProxy ? ` · ${t("editor.proxy_active")}` : ""}
            {dirty ? " · …" : ""}
          </p>
        </div>
        <div className="ml-auto flex items-center gap-1.5">
          <button onClick={() => undo()} disabled={!canUndo()} className={iconBtn} title={t("editor.undo")}>
            <Undo2 size={15} />
          </button>
          <button onClick={() => redo()} disabled={!canRedo()} className={iconBtn} title={t("editor.redo")}>
            <Redo2 size={15} />
          </button>
          <button
            onClick={() => useEditorStore.getState().splitAtPlayhead()}
            className={iconBtn}
            title={`${t("editor.split")} (Ctrl+K)`}
          >
            <Scissors size={15} />
          </button>
          <button
            onClick={() => selectedId && useEditorStore.getState().duplicateSegment(selectedId)}
            disabled={!selectedId}
            className={iconBtn}
            title={`${t("editor.duplicate")} (Ctrl+D)`}
          >
            <Copy size={15} />
          </button>
          <button
            onClick={() => selectedId && useEditorStore.getState().deleteSegment(selectedId)}
            disabled={!selectedId}
            className={iconBtn}
            title={`${t("editor.delete")} (Supr)`}
          >
            <Trash2 size={15} />
          </button>
          <button
            onClick={() => setVisibleMs((v) => Math.max(1000, v / 1.6))}
            className={iconBtn}
            title={t("editor.zoom_in")}
          >
            <ZoomIn size={15} />
          </button>
          <button
            onClick={() => setVisibleMs((v) => Math.min(Math.max(total * 1.2, 4000), v * 1.6))}
            className={iconBtn}
            title={t("editor.zoom_out")}
          >
            <ZoomOut size={15} />
          </button>
          <button
            onClick={() => setShowExport(true)}
            className="ml-1 inline-flex items-center gap-1.5 rounded-lg border border-cyan-500/40 bg-cyan-500/15 px-3 py-1.5 text-xs font-semibold text-cyan-100 transition hover:bg-cyan-500/25"
          >
            <Download size={14} /> {t("editor.export")}
          </button>
        </div>
      </div>

      <div className="flex min-h-0 flex-1">
        {/* Tool rail: gains (E2); text/effects arrive in E3/E4. */}
        <aside className="flex w-44 shrink-0 flex-col gap-3 overflow-y-auto border-r border-white/10 p-3">
          <p className="text-[11px] font-semibold uppercase tracking-wide text-slate-500">
            {t("editor.audio")}
          </p>
          {!selected && <p className="text-[11px] text-slate-600">{t("editor.no_clip")}</p>}
          {selected &&
            gains.map(([key, field, label]) => (
            <label key={key} className="space-y-1 text-[11px] text-slate-400">
              <span className="flex items-center gap-1">
                {(selected[field] as number) > 0 ? <Volume2 size={11} /> : <VolumeX size={11} />}
                {label}
                <span className="ml-auto font-mono">
                  {Math.round((selected[field] as number) * 100)}%
                </span>
              </span>
              <input
                type="range"
                min={0}
                max={200}
                value={Math.round((selected[field] as number) * 100)}
                onChange={(e) =>
                  selectedId &&
                  useEditorStore
                    .getState()
                    .updateSegment(selectedId, { [field]: Number(e.target.value) / 100 })
                }
                className="w-full accent-cyan-400"
              />
            </label>
          ))}
          <p className="mt-auto text-[10px] leading-relaxed text-slate-600">
            {t("editor.shortcuts")}
          </p>
        </aside>

        {/* Preview + transport */}
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex min-h-0 flex-1 items-center justify-center bg-black/50">
            <video
              ref={videoRef}
              src={session.videoUrl}
              onClick={toggle}
              onPlay={() => useEditorStore.getState().setPlaying(true)}
              onPause={() => useEditorStore.getState().setPlaying(false)}
              onEnded={pause}
              className="max-h-full max-w-full cursor-pointer object-contain"
            />
          </div>
          <div className="flex items-center gap-3 border-t border-white/10 px-3 py-2">
            <button onClick={toggle} className={iconBtn} title={playing ? t("editor.pause") : t("editor.play")}>
              {playing ? <Pause size={15} /> : <Play size={15} />}
            </button>
            <TimeLabel total={total} />
            <span className="ml-auto font-mono text-[11px] text-slate-500">
              {project.segments.length} clip{project.segments.length === 1 ? "" : "s"}
            </span>
          </div>
        </div>
      </div>

      {/* Timeline */}
      <div className="h-[190px] shrink-0 overflow-hidden border-t border-white/10 bg-black/30">
        <TimelineView
          audioTracks={session.audioTracks}
          laneRefs={laneRefs}
          visibleEnd={visibleMs}
          onSeek={seek}
        />
      </div>

      {showExport && (
        <ExportDialog session={session} onClose={() => setShowExport(false)} onDone={exportDone} />
      )}
      {toast && (
        <div className="pointer-events-none fixed bottom-6 left-1/2 z-50 -translate-x-1/2 rounded-lg border border-cyan-500/30 bg-cyan-500/15 px-4 py-2 text-sm text-cyan-100">
          {toast}
        </div>
      )}
    </div>
  );
}

function TimeLabel({ total }: { total: number }) {
  const ms = useEditorStore((s) => s.playheadMs);
  return (
    <span className="font-mono text-xs text-slate-400">
      {fmt(ms)} / {fmt(total)}
    </span>
  );
}

function fmt(ms: number) {
  const s = Math.max(0, ms) / 1000;
  const m = Math.floor(s / 60);
  return `${m}:${(s % 60).toFixed(1).padStart(4, "0")}`;
}
