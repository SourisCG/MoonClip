import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { Minus, Square, X } from "lucide-react";
import { MoonClipLogo } from "../logo/MoonClipLogo";
import type { ClipMetadata } from "../../types";

type ResizeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

/** Ignore toggle requests closer than this (double-click fires click+click+dblclick). */
const TOGGLE_DEBOUNCE_MS = 500;

const EDGES: { dir: ResizeDirection; cls: string }[] = [
  { dir: "North", cls: "left-2 right-2 top-0 h-1.5 cursor-ns-resize" },
  { dir: "South", cls: "bottom-0 left-2 right-2 h-1.5 cursor-ns-resize" },
  { dir: "West", cls: "bottom-2 left-0 top-2 w-1.5 cursor-ew-resize" },
  { dir: "East", cls: "bottom-2 right-0 top-2 w-1.5 cursor-ew-resize" },
  { dir: "NorthWest", cls: "left-0 top-0 h-3 w-3 cursor-nwse-resize" },
  { dir: "SouthEast", cls: "bottom-0 right-0 h-3 w-3 cursor-nwse-resize" },
  { dir: "NorthEast", cls: "right-0 top-0 h-3 w-3 cursor-nesw-resize" },
  { dir: "SouthWest", cls: "bottom-0 left-0 h-3 w-3 cursor-nesw-resize" },
];

/** Frameless custom topbar: drag region + brand + window controls (close hides to tray). */
export function Topbar() {
  const { t } = useTranslation();
  // Mirrors the OS state (synced via resize events), never toggled blindly.
  const [maximized, setMaximized] = useState(false);
  const [lastClip, setLastClip] = useState<string | null>(null);
  const lastToggle = useRef(0);

  useEffect(() => {
    let cancelled = false;
    const sync = async () => {
      try {
        const isMax = await getCurrentWindow().isMaximized();
        if (!cancelled) setMaximized(isMax);
      } catch (err) {
        console.error(err);
      }
    };
    void sync();
    let unlistenResize: (() => void) | undefined;
    let unlistenClip: (() => void) | undefined;
    (async () => {
      try {
        const fn = await getCurrentWindow().onResized(() => void sync());
        if (cancelled) fn();
        else unlistenResize = fn;
      } catch (err) {
        console.error(err);
      }
    })();
    (async () => {
      try {
        const fn = await listen<ClipMetadata>("moonclip://clip-saved", (event) => {
          const name = event.payload.file_name;
          setLastClip(name);
          void getCurrentWindow()
            .setTitle(`MoonClip — ${name}`)
            .catch(console.error);
        });
        if (cancelled) fn();
        else unlistenClip = fn;
      } catch (err) {
        console.error(err);
      }
    })();
    return () => {
      cancelled = true;
      unlistenResize?.();
      unlistenClip?.();
    };
  }, []);

  const win = () => getCurrentWindow();
  const stop = (e: React.SyntheticEvent) => e.stopPropagation();

  const onMinimize = (e: React.MouseEvent) => {
    stop(e);
    void win().minimize();
  };

  const doToggle = async () => {
    const w = win();
    try {
      // Atomic toggle: no check-then-act race between rapid invocations.
      await w.toggleMaximize();
      setMaximized(await w.isMaximized());
    } catch (err) {
      console.error(err);
    }
  };

  /**
   * Buttons only. Double-click on the drag area is handled NATIVELY by
   * Tauri (internal-toggle-maximize, see tauri#12006) — adding our own
   * dblclick handler double-toggles (maximize then unmaximize flicker).
   */
  const onToggleMax = (e: React.MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
    const now = Date.now();
    if (now - lastToggle.current < TOGGLE_DEBOUNCE_MS) return;
    lastToggle.current = now;
    void doToggle();
  };
  const onClose = (e: React.MouseEvent) => {
    stop(e);
    // CloseRequested is intercepted in Rust -> hide to tray.
    void win().close();
  };
  const onResizeStart = (dir: ResizeDirection) => (e: React.PointerEvent) => {
    e.stopPropagation();
    e.preventDefault();
    if (maximized) return;
    void win().startResizeDragging(dir).catch(console.error);
  };

  return (
    <>
      {/* Resize grips (frameless window has no native ones). Hidden when maximized. */}
      {!maximized &&
        EDGES.map(({ dir, cls }) => (
          <div
            key={dir}
            onPointerDown={onResizeStart(dir)}
            className={`fixed z-30 touch-none select-none ${cls}`}
          />
        ))}
      <header className="relative z-20 flex h-10 select-none items-center justify-between border-b border-white/5 bg-[#0d0f14]/85 pl-3 pr-2 backdrop-blur-md">
        <div data-tauri-drag-region className="flex h-full min-w-0 flex-1 items-center gap-2">
          <MoonlitLogo size={18} />
          <span className="shrink-0 text-sm font-extrabold tracking-wide text-slate-100">
            Moon<span className="text-indigo-400">Lit</span>
          </span>
          <span className="hidden truncate text-[11px] text-slate-500 sm:inline">
            {lastClip ?? t("app.tagline")}
          </span>
        </div>
        <div className="flex items-center gap-1" onDoubleClick={stop}>
          <button
            aria-label={t("window.minimize")}
            title={t("window.minimize")}
            onPointerDown={stop}
            onClick={onMinimize}
            className="rounded-md p-1.5 text-slate-400 transition hover:bg-white/10 hover:text-slate-100"
          >
            <Minus size={15} />
          </button>
          <button
            aria-label={maximized ? t("window.restore") : t("window.maximize")}
            title={maximized ? t("window.restore") : t("window.maximize")}
            onPointerDown={stop}
            onClick={(e) => onToggleMax(e)}
            className="rounded-md p-1.5 text-slate-400 transition hover:bg-white/10 hover:text-slate-100"
          >
            <Square size={13} />
          </button>
          <button
            aria-label={t("window.close")}
            title={t("window.close")}
            onPointerDown={stop}
            onClick={onClose}
            className="rounded-md p-1.5 text-slate-400 transition hover:bg-red-500/80 hover:text-white"
          >
            <X size={15} />
          </button>
        </div>
      </header>
    </>
  );
}
