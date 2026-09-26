import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { Circle, Clapperboard, Gamepad2, Settings, Square } from "lucide-react";
import { MoonClipStarfield } from "./components/starfield/MoonClipStarfield";
import { MoonClipLogo } from "./components/logo/MoonClipLogo";
import { Topbar } from "./components/topbar/Topbar";
import { SettingsModal } from "./components/settings/SettingsModal";
import { SetupWizard } from "./components/settings/SetupWizard";
import { AppManager } from "./components/settings/AppManager";
import { GalleryView } from "./components/gallery/GalleryView";
import { GamePrompt } from "./components/GamePrompt";
import { useClips } from "./hooks/useClips";
import { useEngine } from "./hooks/useEngine";
import { useLocale } from "./hooks/useLocale";
import { useCurrentGame } from "./hooks/useCurrentGame";

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
  const { status, busy, error: engineError, start, stop, saveNow } = useEngine(onClipSaved);
  const [hotkey, setHotkey] = useState("F9");
  const [showWizard, setShowWizard] = useState(false);

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
    <div className="relative flex h-screen flex-col overflow-hidden bg-moonclip-void font-sans text-slate-100 selection:bg-cyan-500/30">
      <Topbar />
      <div className="relative flex min-h-0 flex-1">
        <MoonClipStarfield />
        <div className="pointer-events-none fixed left-1/2 top-10 h-[250px] w-[min(700px,100vw)] -translate-x-1/2 bg-gradient-to-b from-cyan-500/10 via-indigo-500/5 to-transparent blur-3xl" />

        <div className="relative z-10 flex min-h-0 flex-1 gap-2 overflow-x-auto p-2 sm:gap-4 sm:p-4">
          <aside className="flex max-h-[calc(100vh-4rem)] w-14 shrink-0 flex-col justify-between self-start overflow-y-auto rounded-2xl border border-white/5 bg-moonclip-panel/50 p-2 shadow-2xl backdrop-blur-xl sm:max-h-[calc(100vh-5.5rem)] lg:w-56 lg:p-4 xl:w-64">
            <div>
              <div className="mb-3 flex items-center justify-center gap-2.5 px-0 py-2 lg:mb-6 lg:justify-start lg:px-2 lg:py-3">
                <MoonClipLogo size={30} />
                <h1 className="hidden bg-gradient-to-r from-white via-slate-200 to-slate-400 bg-clip-text text-lg font-bold tracking-wider text-transparent lg:block">
                  {t("app.name")}
                </h1>
              </div>

              <nav className="space-y-1">
                {(
                  [
                    { id: "clips", icon: <Clapperboard size={15} />, label: t("nav.clips") },
                    { id: "games", icon: <Gamepad2 size={15} />, label: t("nav.games") },
                    { id: "settings", icon: <Settings size={15} />, label: t("nav.settings") },
                  ] as { id: View; icon: React.ReactNode; label: string }[]
                ).map((item) => (
                  <button
                    key={item.id}
                    onClick={() => setView(item.id)}
                    title={item.label}
                    className={
                      view === item.id
                        ? "flex w-full items-center justify-center rounded-xl border border-cyan-500/20 bg-cyan-500/10 px-0 py-2 text-sm font-medium text-cyan-300 shadow-[0_0_10px_rgba(56,189,248,0.1)] lg:justify-start lg:px-3"
                        : "flex w-full items-center justify-center rounded-xl px-0 py-2 text-sm font-medium text-slate-400 transition hover:bg-white/5 hover:text-slate-200 lg:justify-start lg:px-3"
                    }
                  >
                    <span className="inline-flex items-center gap-2">
                      {item.icon}
                      <span className="hidden lg:inline">{item.label}</span>
                    </span>
                  </button>
                ))}
              </nav>
            </div>

            <div className="space-y-3">
              <div className="rounded-xl border border-white/5 bg-[#0f1424]/80 p-2 lg:p-3">
                <div className="flex items-center justify-center gap-2 lg:justify-start">
                  <span className="relative flex h-2.5 w-2.5">
                    {status.running ? (
                      <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-red-500" />
                    ) : (
                      <>
                        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-cyan-400 opacity-75" />
                        <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-cyan-500" />
                      </>
                    )}
                  </span>
                  <div className="hidden text-xs lg:block">
                    <p className="font-medium text-slate-300">
                      {status.running ? t("rec.recording") : t("status.standby")}
                    </p>
                    <p className="truncate text-[11px] text-slate-500">
                      {status.running
                        ? `${currentGame ? currentGame.title : status.backend} · ${t("rec.tracks", { n: status.tracks_linked })}`
                        : currentGame
                          ? t("game.waiting", { name: currentGame.title })
                          : t("status.shortcut", { hotkey })}
                    </p>
                  </div>
                </div>
                <button
                  onClick={() => void (status.running ? stop() : start())}
                  disabled={busy}
                  title={status.running ? t("rec.stop") : t("rec.start")}
                  className={`mt-2 inline-flex w-full items-center justify-center gap-2 rounded-lg border px-2 py-1.5 text-xs font-semibold transition disabled:opacity-50 lg:mt-2.5 ${
                    status.running
                      ? "border-red-500/40 bg-red-500/10 text-red-300 hover:bg-red-500/20"
                      : "border-cyan-500/40 bg-cyan-500/10 text-cyan-200 hover:bg-cyan-500/20"
                  }`}
                >
                  {status.running ? <Square size={12} /> : <Circle size={12} />}
                  <span className="hidden lg:inline">
                    {status.running ? t("rec.stop") : t("rec.start")}
                  </span>
                </button>
                {engineError && (
                  <p className="mt-1.5 break-all font-mono text-[11px] text-red-400">{engineError}</p>
                )}
                {status.engine_error && (
                  <p className="mt-1.5 break-all font-mono text-[11px] text-amber-400">
                    {status.engine_error}
                  </p>
                )}
              </div>
              <div className="hidden items-center justify-between rounded-xl border border-white/5 bg-black/30 px-3 py-2 text-xs lg:flex">
                <span className="text-slate-400">{t("lang.label")}</span>
                <div className="flex gap-1">
                  <button
                    onClick={() => void setLocale("es")}
                    className={`rounded-md px-2 py-1 font-mono ${locale.startsWith("es") ? "bg-cyan-500/20 text-cyan-200" : "text-slate-500 hover:text-slate-200"}`}
                  >
                    {t("lang.es")}
                  </button>
                  <button
                    onClick={() => void setLocale("en")}
                    className={`rounded-md px-2 py-1 font-mono ${locale.startsWith("en") ? "bg-cyan-500/20 text-cyan-200" : "text-slate-500 hover:text-slate-200"}`}
                  >
                    {t("lang.en")}
                  </button>
                </div>
              </div>
            </div>
          </aside>

          <main className="min-h-0 min-w-0 flex-1 overflow-y-auto rounded-2xl border border-white/5 bg-moonclip-panel/30 p-3 shadow-2xl backdrop-blur-xl sm:p-4 lg:p-6">
            <GamePrompt />
            {view === "settings" && (
              <>
                <h2 className="text-xl font-bold text-slate-100">{t("nav.settings")}</h2>
                <div className="mt-4">
                  <SettingsModal
                    engineStatus={status}
                    onHotkeyChange={setHotkey}
                    onOpenWizard={() => setShowWizard(true)}
                  />
                </div>
              </>
            )}
            {view === "games" && (
              <>
                <h2 className="text-xl font-bold text-slate-100">{t("nav.games")}</h2>
                <div className="mt-4">
                  <AppManager />
                </div>
              </>
            )}
            {view === "clips" && (
              <>
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <h2 className="text-xl font-bold text-slate-100">
                    {t("gallery.title")}{" "}
                    <span className="font-mono text-sm font-normal text-slate-500">
                      ({clips.length})
                    </span>
                  </h2>
                  {status.running && (
                    <button
                      onClick={() => void saveNow()}
                      disabled={busy}
                      className="rounded-lg border border-cyan-500/30 bg-cyan-500/10 px-3 py-1.5 text-xs font-semibold text-cyan-200 transition hover:bg-cyan-500/20 disabled:opacity-50"
                    >
                      {t("rec.save_now")}
                    </button>
                  )}
                </div>
                <GalleryView refreshToken={galleryTick} />
              </>
            )}
          </main>
        </div>
      </div>
      {showWizard && <SetupWizard onClose={() => setShowWizard(false)} />}
    </div>
  );
}
