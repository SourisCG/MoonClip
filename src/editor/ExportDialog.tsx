import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { X } from "lucide-react";
import { useEditorStore } from "./store";
import type { EditProgress, EditorOpenResult } from "./types";

const RESOLUTIONS = [0, 1080, 720, 480];
const ASPECTS = ["source", "16:9", "9:16", "1:1"];
const FPS = [0, 30, 60];
const BITRATES = [0, 4000, 6000, 10000, 16000, 20000];

/** Export dialog: presets + progress + cancel (one export at a time). */
export function ExportDialog({
  session,
  onClose,
  onDone,
}: {
  session: EditorOpenResult;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useTranslation();
  const output = useEditorStore((s) => s.project?.output);
  const setOutput = useEditorStore((s) => s.setOutput);
  const [percent, setPercent] = useState(0);
  const [stage, setStage] = useState("");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let unProgress: (() => void) | undefined;
    let unDone: (() => void) | undefined;
    void listen<EditProgress>("moonclip://edit-progress", (e) => {
      if (e.payload.op !== "export" || e.payload.sessionId !== session.sessionId) return;
      setPercent(e.payload.percent);
      setStage(e.payload.stage ?? "");
    }).then((u) => {
      unProgress = u;
    });
    void listen<{ ok: boolean; error: string | null }>("moonclip://editor-export-done", (e) => {
      setRunning(false);
      if (e.payload.ok) {
        setPercent(100);
        onDone();
      } else {
        setError(e.payload.error ?? t("editor.export_failed"));
      }
    }).then((u) => {
      unDone = u;
    });
    return () => {
      unProgress?.();
      unDone?.();
    };
  }, [session.sessionId, onDone, t]);

  const start = async () => {
    const project = useEditorStore.getState().project;
    if (!project) return;
    setRunning(true);
    setError(null);
    setPercent(0);
    try {
      await invoke("editor_export", { sessionId: session.sessionId, project });
    } catch (e) {
      setRunning(false);
      setError(String(e));
    }
  };

  const cancel = async () => {
    try {
      await invoke("editor_cancel_export", { sessionId: session.sessionId });
    } catch {
      /* already finished */
    }
    setRunning(false);
  };

  const select =
    "rounded-control border border-line bg-raised/60 px-2 py-1 text-xs text-ink outline-none focus:border-link/60";

  if (!output) return null;
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 p-4">
      <div className="w-full max-w-lg rounded-card border border-line bg-surface p-5 shadow-panel">
        <div className="mb-4 flex items-center">
          <h3 className="text-sm font-semibold text-ink">{t("editor.export_title")}</h3>
          <button
            onClick={running ? undefined : onClose}
            disabled={running}
            className="ml-auto rounded-control p-1.5 text-ink-faint transition hover:bg-raised hover:text-ink disabled:opacity-40"
          >
            <X size={16} />
          </button>
        </div>

        <div className="grid grid-cols-2 gap-3 text-xs">
          <label className="space-y-1">
            <span className="text-ink-muted">{t("editor.resolution")}</span>
            <select
              className={`${select} w-full`}
              value={output.height}
              disabled={running}
              onChange={(e) => setOutput({ height: Number(e.target.value) })}
            >
              {RESOLUTIONS.map((r) => (
                <option key={r} value={r}>
                  {r === 0 ? t("editor.source") : `${r}p`}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1">
            <span className="text-ink-muted">{t("editor.aspect")}</span>
            <select
              className={`${select} w-full`}
              value={output.aspect}
              disabled={running}
              onChange={(e) => setOutput({ aspect: e.target.value })}
            >
              {ASPECTS.map((a) => (
                <option key={a} value={a}>
                  {a === "source" ? t("editor.source") : a}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1">
            <span className="text-ink-muted">{t("editor.fps")}</span>
            <select
              className={`${select} w-full`}
              value={output.fps}
              disabled={running}
              onChange={(e) => setOutput({ fps: Number(e.target.value) })}
            >
              {FPS.map((f) => (
                <option key={f} value={f}>
                  {f === 0 ? t("editor.source") : `${f}`}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1">
            <span className="text-ink-muted">{t("editor.bitrate")}</span>
            <select
              className={`${select} w-full`}
              value={output.bitrateKbps}
              disabled={running}
              onChange={(e) => setOutput({ bitrateKbps: Number(e.target.value) })}
            >
              {BITRATES.map((b) => (
                <option key={b} value={b}>
                  {b === 0 ? t("editor.auto") : `${b / 1000} Mbps`}
                </option>
              ))}
            </select>
          </label>
          <label className="space-y-1">
            <span className="text-ink-muted">{t("editor.encoder")}</span>
            <select
              className={`${select} w-full`}
              value={output.encoder}
              disabled={running}
              onChange={(e) => setOutput({ encoder: e.target.value })}
            >
              <option value="auto">{t("editor.encoder_auto")}</option>
              <option value="cpu">x264 (CPU)</option>
              {session.encoders
                .filter((e) => e.hw && e.available)
                .map((e) => (
                  <option key={e.id} value={e.id}>
                    {e.label}
                  </option>
                ))}
            </select>
          </label>
          <label className="space-y-1">
            <span className="text-ink-muted">{t("editor.container")}</span>
            <select
              className={`${select} w-full`}
              value={output.container}
              disabled={running}
              onChange={(e) => setOutput({ container: e.target.value })}
            >
              <option value="mp4">MP4</option>
              <option value="mkv">MKV</option>
            </select>
          </label>
          <label className="space-y-1">
            <span className="text-ink-muted">{t("editor.audio_out")}</span>
            <select
              className={`${select} w-full`}
              value={output.audio ?? "mix"}
              disabled={running}
              onChange={(e) => setOutput({ audio: e.target.value })}
            >
              <option value="mix">{t("editor.audio_out_mix")}</option>
              <option value="tracks">{t("editor.audio_out_tracks")}</option>
            </select>
          </label>
        </div>

        {(running || percent > 0) && (
          <div className="mt-4">
            <div className="h-1.5 overflow-hidden rounded-full bg-raised">
              <div
                className="h-full bg-link transition-[width] duration-200"
                style={{ width: `${Math.min(100, percent)}%` }}
              />
            </div>
            <p className="mt-1 text-[11px] text-ink-muted">
              {running ? `${t("editor.exporting")} · ${stage}` : `${Math.round(percent)}%`}
            </p>
          </div>
        )}
        {error && <p className="mt-3 break-words font-mono text-xs text-brand-bright">{error}</p>}

        <div className="mt-5 flex justify-end gap-2">
          {running ? (
            <button
              onClick={() => void cancel()}
              className="rounded-control border border-brand/40 bg-brand/10 px-3 py-1.5 text-sm text-brand-bright transition hover:bg-brand/20"
            >
              {t("editor.cancel")}
            </button>
          ) : (
            <>
              <button
                onClick={onClose}
                className="rounded-control border border-line bg-raised/60 px-3 py-1.5 text-sm text-ink transition hover:bg-raised"
              >
                {t("editor.close")}
              </button>
              <button
                onClick={() => void start()}
                disabled={!useEditorStore.getState().project?.segments.length}
                className="rounded-control border border-link/40 bg-link/10 px-3 py-1.5 text-sm font-semibold text-link-bright transition hover:bg-link/20 disabled:opacity-50"
              >
                {t("editor.start_export")}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
