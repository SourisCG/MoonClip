import { Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { Circle, Clapperboard, Gamepad2, Monitor, Settings, Square } from "lucide-react";
import { MoonClipStarfield } from "./components/starfield/MoonClipStarfield";
import { MoonClipLogo } from "./components/logo/MoonClipLogo";
import { Topbar } from "./components/topbar/Topbar";
import { SettingsModal } from "./components/settings/SettingsModal";
import { SetupWizard } from "./components/settings/SetupWizard";
import { AppManager } from "./components/settings/AppManager";
import { GalleryView } from "./components/gallery/GalleryView";
import { useClips } from "./hooks/useClips";
import { useEngine } from "./hooks/useEngine";
import { useLocale } from "./hooks/useLocale";
import { useCurrentGame } from "./hooks/useCurrentGame";
import type { ClipMetadata } from "./types";

// Heavy editor: separate chunk, only downloaded when the user opens it.
const EditorApp = lazy(() => import("./editor/EditorApp"));

type View = "clips" | "games" | "settings";

export default function App() {
  const { t } = useTranslation();
  const { locale, setLocale } = useLocale();
  const currentGame = useCurrentGame();
  const [view, setView] = useState<View>("clips");
  const { clips, refresh: refreshClips } = useClips();
  const [galleryTick, setGalleryTick] = useState(0);
  const onClipSaved = useCallback(() => {
    setGalleryTick((n) => n + 1);
    void refreshClips();
  }, [refreshClips]);

  // Drive library recovery (auto on connect + manual): refresh quietly when
  // the sync finishes so restored cloud clips show up immediately.
  useEffect(() => {
    const unlisten = listen("moonclip://drive-sync-done", () => {
      onClipSaved();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [onClipSaved]);
  const { status, busy, error: engineError, start, stop, saveNow } = useEngine(onClipSaved);
  const [screenBusy, setScreenBusy] = useState(false);
  const startScreen = async () => {
    setScreenBusy(true);
    try {
      await invoke("start_screen_buffer");
    } catch (e) {
      console.error("record screen:", e);
    } finally {
      setScreenBusy(false);
    }
  };
  const [hotkey, setHotkey] = useState("F9");
  const [showWizard, setShowWizard] = useState(false);
  const [editorClip, setEditorClip] = useState<ClipMetadata | null>(null);
  const editorWasMaximized = useRef(false);

  const openAdvancedEditor = useCallback(async (clip: ClipMetadata) => {
    try {
      const w = getCurrentWindow();
      editorWasMaximized.current = await w.isMaximized();
      if (!editorWasMaximized.current) await w.maximize();
      // The editor needs room for rail + preview + timeline; below this the
      // layout gets cramped (and the timeline unreadable).
      await w.setMinSize(new LogicalSize(1024, 600));
    } catch {
      // window ops are best effort; the editor still opens
    }
    setEditorClip(clip);
  }, []);

  const closeAdvancedEditor = useCallback(async () => {
    try {
      await getCurrentWindow().setMinSize(new LogicalSize(420, 420));
    } catch {
      // best effort
    }
    if (!editorWasMaximized.current) {
      await getCurrentWindow().unmaximize().catch(() => {});
    }
    setEditorClip(null);
    setGalleryTick((n) => n + 1);
    void refreshClips();
  }, [refreshClips]);

  // First real paint + compositing mode: the DMA-BUF self-healing marks the
  // accelerated path as working, and software compositing switches the UI to
  // a low-power mode (no backdrop blur, no starfield).
  useEffect(() => {
    void invoke("first_paint").catch(() => {});
    invoke<boolean>("compositing_status")
      .then((software) =>
        document.documentElement.classList.toggle("low-power", software),
      )
      .catch(() => {});
  }, []);

  useEffect(() => {
    let cancelled = false;
    invoke<Record<string, string>>("get_settings")
      .then((s) => {
        if (!cancelled && s.setup_done !== "1") setShowWizard(true);
      })
      .catch(() => {
        // first-run check is best effort
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    invoke<string>("get_hotkey")
      .then((h) => {
        if (!cancelled) setHotkey(h);
      })
      .catch(() => {
        if (!cancelled) setHotkey("F9");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="relative flex h-screen flex-col overflow-hidden bg-void font-sans text-ink selection:bg-blood/35">
      <Topbar />
      {editorClip ? (
        <Suspense
          fallback={
            <div className="flex min-h-0 flex-1 items-center justify-center font-mono text-xs uppercase tracking-[0.2em] text-ink-muted">
              {t("editor.loading")}
            </div>
          }
        >
          <EditorApp
            clipId={editorClip.id}
            onExit={() => void closeAdvancedEditor()}
            onClipSaved={onClipSaved}
          />
        </Suspense>
      ) : (
      <div className="relative flex min-h-0 flex-1">
        <MoonClipStarfield />
        <div className="pointer-events-none fixed left-1/2 top-8 h-[260px] w-[min(720px,100vw)] -translate-x-1/2 bg-gradient-to-b from-lava/10 via-gold/5 to-transparent blur-3xl" />
        <div className="pointer-events-none fixed -left-24 bottom-0 h-[240px] w-[420px] bg-gradient-to-tr from-sky/10 via-aether/5 to-transparent blur-3xl" />

        <div className="relative z-10 flex min-h-0 flex-1 gap-2 overflow-x-auto p-2 sm:gap-4 sm:p-4">
          <aside className="flex max-h-[calc(100vh-4rem)] w-14 shrink-0 flex-col justify-between self-start overflow-y-auto rounded-card border border-line bg-panel/60 p-2 shadow-panel backdrop-blur-xl sm:max-h-[calc(100vh-5.5rem)] lg:w-56 lg:p-3 xl:w-60">
            <div>
              <div className="mb-3 flex items-center justify-center gap-2.5 px-0 py-2 lg:mb-5 lg:justify-start lg:px-1 lg:py-2">
                <MoonClipLogo size={30} />
                <h1 className="hidden font-display text-lg font-bold tracking-wide text-ink lg:block">
                  {t("app.name")}
                </h1>
              </div>

              <nav className="space-y-1">
                {(
                  [
                    { id: "clips", num: "01", icon: <Clapperboard size={15} />, label: t("nav.clips") },
                    { id: "games", num: "02", icon: <Gamepad2 size={15} />, label: t("nav.games") },
                    { id: "settings", num: "03", icon: <Settings size={15} />, label: t("nav.settings") },
                  ] as { id: View; num: string; icon: React.ReactNode; label: string }[]
                ).map((item) => (
                  <button
                    key={item.id}
                    onClick={() => setView(item.id)}
                    title={item.label}
                    className={
                      view === item.id
                        ? "flex w-full items-center justify-center gap-2 rounded-stamp border border-ink bg-paper px-0 py-1.5 text-sm font-semibold text-ink shadow-stamp-blood lg:justify-start lg:px-2.5"
                        : "flex w-full items-center justify-center gap-2 rounded-stamp border border-transparent px-0 py-1.5 text-sm font-medium text-ink-muted transition hover:bg-raised/70 hover:text-ink lg:justify-start lg:px-2.5"
                    }
                  >
                    <span
                      className={
                        "hidden font-mono text-[10px] tracking-widest lg:inline " +
                        (view === item.id ? "text-blood" : "text-ink-faint")
                      }
                    >
                      {item.num}
                    </span>
                    {item.icon}
                    <span className="hidden lg:inline">{item.label}</span>
                  </button>
                ))}
              </nav>
            </div>

            <div className="space-y-3">
              {/* Status stamp card: the fanzine "editor card" with the live state. */}
              <div className="relative rounded-card border-2 border-ink bg-paper p-2 text-ink shadow-stamp lg:p-3">
                <span
                  aria-hidden
                  className="pointer-events-none absolute inset-1 rounded-[6px] border border-dashed border-ink/25"
                />
                <div className="relative flex items-center justify-center gap-2 lg:justify-start">
                  <span className="relative flex h-2.5 w-2.5">
                    {status.running ? (
                      <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-lava shadow-[0_0_8px_rgba(232,115,41,0.8)]" />
                    ) : (
                      <>
                        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-jade opacity-60" />
                        <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-jade" />
                      </>
                    )}
                  </span>
                  <div className="hidden text-xs lg:block">
                    <p className="font-mono text-[10px] font-bold uppercase tracking-[0.2em] text-blood">
                      {status.running ? t("rec.recording") : t("status.standby")}
                    </p>
                    <p className="truncate text-[11px] text-ink/70">
                      {status.running
                        ? `${currentGame ?? status.backend} · ${t("rec.tracks", { n: status.tracks_linked })}`
                        : currentGame
                          ? t("game.running", { name: currentGame })
                          : t("status.shortcut", { hotkey })}
                    </p>
                  </div>
                </div>
                <button
                  onClick={() => void (status.running ? stop() : start())}
                  disabled={busy}
                  title={status.running ? t("rec.stop") : t("rec.start")}
                  className={`relative mt-2 inline-flex w-full items-center justify-center gap-2 rounded-stamp border-2 px-2 py-1.5 text-xs font-bold uppercase tracking-wider transition disabled:opacity-50 lg:mt-2.5 ${
                    status.running
                      ? "border-blood bg-blood/10 text-blood hover:bg-blood hover:text-paper"
                      : "border-ink bg-blood text-paper hover:bg-blood-bright"
                  }`}
                >
                  {status.running ? <Square size={12} /> : <Circle size={12} />}
                  <span className="hidden lg:inline">
                    {status.running ? t("rec.stop") : t("rec.start")}
                  </span>
                </button>
                {!status.running && (
                  <button
                    onClick={() => void startScreen()}
                    disabled={screenBusy}
                    title={t("rec.record_screen")}
                    className="relative mt-2 inline-flex w-full items-center justify-center gap-2 rounded-stamp border border-dashed border-ink/50 bg-transparent px-2 py-1.5 text-[11px] font-semibold text-ink/80 transition hover:border-ink hover:text-ink disabled:opacity-50"
                  >
                    <Monitor size={12} />
                    <span className="hidden lg:inline">{t("rec.record_screen")}</span>
                  </button>
                )}
                {engineError && (
                  <p className="relative mt-1.5 break-all font-mono text-[11px] text-blood">
                    {engineError}
                  </p>
                )}
                {status.engine_error && (
                  <p className="relative mt-1.5 break-all font-mono text-[11px] text-gold-bright">
                    {status.engine_error}
                  </p>
                )}
              </div>
              <div className="hidden items-center justify-between rounded-stamp border border-line bg-void/50 px-3 py-2 text-xs lg:flex">
                <span className="font-mono text-[10px] uppercase tracking-[0.16em] text-ink-faint">
                  {t("lang.label")}
                </span>
                <div className="flex gap-1">
                  <button
                    onClick={() => void setLocale("es")}
                    className={`rounded-stamp px-2 py-1 font-mono text-[11px] ${locale.startsWith("es") ? "bg-paper text-ink" : "text-ink-muted hover:text-ink"}`}
                  >
                    {t("lang.es")}
                  </button>
                  <button
                    onClick={() => void setLocale("en")}
                    className={`rounded-stamp px-2 py-1 font-mono text-[11px] ${locale.startsWith("en") ? "bg-paper text-ink" : "text-ink-muted hover:text-ink"}`}
                  >
                    {t("lang.en")}
                  </button>
                </div>
              </div>
            </div>
          </aside>

          <main className="min-h-0 min-w-0 flex-1 overflow-y-auto rounded-card border border-line bg-panel/40 p-3 shadow-panel backdrop-blur-xl sm:p-4 lg:p-6">
            {view === "settings" && (
              <>
                <div className="mb-4 border-b-2 border-line pb-2">
                  <h2 className="font-display text-xl font-bold text-ink">{t("nav.settings")}</h2>
                </div>
                <SettingsModal
                  engineStatus={status}
                  onHotkeyChange={setHotkey}
                  onOpenWizard={() => setShowWizard(true)}
                />
              </>
            )}
            {view === "games" && (
              <>
                <div className="mb-4 border-b-2 border-line pb-2">
                  <h2 className="font-display text-xl font-bold text-ink">{t("nav.games")}</h2>
                </div>
                <AppManager />
              </>
            )}
            {view === "clips" && (
              <>
                <div className="mb-3 flex flex-wrap items-center justify-between gap-2 border-b-2 border-line pb-2">
                  <h2 className="font-display text-xl font-bold text-ink">
                    {t("gallery.title")}{" "}
                    <span className="font-mono text-sm font-normal text-ink-faint">
                      ({clips.length})
                    </span>
                  </h2>
                  {status.running && (
                    <button
                      onClick={() => void saveNow()}
                      disabled={busy}
                      className="inline-flex items-center gap-2 rounded-stamp border-2 border-ink bg-blood px-3 py-1.5 text-xs font-bold uppercase tracking-wider text-paper transition hover:bg-blood-bright disabled:opacity-50"
                    >
                      {t("rec.save_now")}
                    </button>
                  )}
                </div>
                <GalleryView refreshToken={galleryTick} onAdvancedEdit={(c) => void openAdvancedEditor(c)} />
              </>
            )}
          </main>
        </div>
      </div>
      )}
      {showWizard && <SetupWizard onClose={() => setShowWizard(false)} />}
    </div>
  );
}
