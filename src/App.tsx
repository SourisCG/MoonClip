import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { LogicalSize } from "@tauri-apps/api/dpi";
import {
  Circle,
  Cloud,
  Clapperboard,
  Gamepad2,
  HardDriveDownload,
  Library,
  Monitor,
  Search,
  Settings,
  Square,
  Star,
  Trash2,
  Upload,
} from "lucide-react";
import { MoonClipStarfield } from "./components/starfield/MoonClipStarfield";
import { MoonClipLogo } from "./components/logo/MoonClipLogo";
import { Topbar } from "./components/topbar/Topbar";
import { SettingsModal } from "./components/settings/SettingsModal";
import { SetupWizard } from "./components/settings/SetupWizard";
import { AppManager } from "./components/settings/AppManager";
import { GalleryView } from "./components/gallery/GalleryView";
import { DriveBrowser } from "./components/gallery/DriveBrowser";
import { IconRail, ContextRail, RailButton, ShellHeader, ShellMain } from "./components/shell/AppShell";
import { Select } from "./components/ui/Select";
import { Button } from "./components/ui/Button";
import { useClips } from "./hooks/useClips";
import { useEngine } from "./hooks/useEngine";
import { useLocale } from "./hooks/useLocale";
import { useCurrentGame } from "./hooks/useCurrentGame";
import { useRegisteredInputs } from "./hooks/useRegisteredInputs";
import type { ClipMetadata } from "./types";

// Heavy editor: separate chunk, only downloaded when the user opens it.
const EditorApp = lazy(() => import("./editor/EditorApp"));

type View = "clips" | "games" | "settings";

const GROUP_KEY = "moonclip.gallery.group";
const SORT_KEY = "moonclip.gallery.sort";
const FLAT = "__flat__";

export default function App() {
  const { t } = useTranslation();
  const { locale, setLocale } = useLocale();
  const currentGame = useCurrentGame();
  const [view, setView] = useState<View>("clips");
  const { clips, refresh: refreshClips, purgeMissing } = useClips();
  const { inputs } = useRegisteredInputs();
  const [galleryTick, setGalleryTick] = useState(0);
  const onClipSaved = useCallback(() => {
    setGalleryTick((n) => n + 1);
    void refreshClips();
  }, [refreshClips]);

  // Gallery filtering lives here: the contextual rail and the header own the
  // controls, the grid consumes the result.
  const [group, setGroupState] = useState<string>(
    () => localStorage.getItem(GROUP_KEY) ?? "all",
  );
  const [query, setQuery] = useState("");
  const [sort, setSortState] = useState<string>(
    () => localStorage.getItem(SORT_KEY) ?? "recent",
  );
  const setGroup = (key: string) => {
    setGroupState(key);
    localStorage.setItem(GROUP_KEY, key);
  };
  const setSort = (value: string) => {
    setSortState(value);
    localStorage.setItem(SORT_KEY, value);
  };
  const [showDrive, setShowDrive] = useState(false);
  const [purgeMsg, setPurgeMsg] = useState<string | null>(null);

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

  const gameGroups = useMemo(() => {
    const map = new Map<string, number>();
    for (const clip of clips) {
      const key = clip.folder || FLAT;
      map.set(key, (map.get(key) ?? 0) + 1);
    }
    return [...map.entries()].map(([key, count]) => ({ key, count }));
  }, [clips]);

  const labelFor = useCallback(
    (key: string) => {
      if (key === FLAT) return t("gallery.unfiled");
      if (key === "Unknown") return t("gallery.no_game");
      return inputs.find((i) => i.clips_folder === key)?.display_name ?? key;
    },
    [inputs, t],
  );

  const counts = useMemo(
    () => ({
      all: clips.length,
      favorites: clips.filter((c) => c.is_favorite).length,
      uploaded: clips.filter((c) => !!c.drive_file_id).length,
      cloud: clips.filter((c) => c.cloud).length,
    }),
    [clips],
  );

  const onPurge = () => {
    setPurgeMsg(null);
    purgeMissing().then(
      (n) => setPurgeMsg(t("gallery.purged", { count: n })),
      (e) => setPurgeMsg(String(e)),
    );
  };

  const railItems = [
    { id: "clips", icon: <Library size={17} />, label: t("nav.clips") },
    { id: "games", icon: <Gamepad2 size={17} />, label: t("nav.games") },
    { id: "settings", icon: <Settings size={17} />, label: t("nav.settings") },
  ];

  const contextRail =
    view === "clips" ? (
      <ContextRail title={t("gallery.title")}>
        <RailButton
          active={group === "all"}
          icon={<Clapperboard size={14} />}
          label={t("gallery.all")}
          count={counts.all}
          onClick={() => setGroup("all")}
        />
        <RailButton
          active={group === "favorites"}
          icon={<Star size={14} />}
          label={t("gallery.favorites")}
          count={counts.favorites}
          onClick={() => setGroup("favorites")}
        />
        <RailButton
          active={group === "uploaded"}
          icon={<Upload size={14} />}
          label={t("gallery.uploaded_section")}
          count={counts.uploaded}
          onClick={() => setGroup("uploaded")}
        />
        <RailButton
          active={group === "cloud"}
          icon={<Cloud size={14} />}
          label={t("gallery.cloud_section")}
          count={counts.cloud}
          onClick={() => setGroup("cloud")}
        />
        <p className="mb-1 mt-3 px-2 font-mono text-[10px] uppercase tracking-[0.18em] text-ink-faint">
          {t("gallery.games")}
        </p>
        {gameGroups.map((entry) => (
          <RailButton
            key={entry.key}
            active={group === entry.key}
            icon={<Gamepad2 size={14} />}
            label={labelFor(entry.key)}
            count={entry.count}
            onClick={() => setGroup(entry.key)}
          />
        ))}
      </ContextRail>
    ) : view === "games" ? (
      <ContextRail title={t("nav.games")}>
        {inputs.map((input) => (
          <RailButton
            key={input.id}
            icon={<Gamepad2 size={14} />}
            label={input.display_name}
            onClick={() => {
              if (input.clips_folder) setGroup(input.clips_folder);
              setView("clips");
            }}
          />
        ))}
        {!inputs.length && (
          <p className="px-2 py-1 text-xs text-ink-faint">{t("games.empty")}</p>
        )}
      </ContextRail>
    ) : null;

  const header =
    view === "clips" ? (
      <>
        <div className="relative w-full max-w-sm">
          <Search
            size={14}
            className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-faint"
          />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("gallery.search")}
            className="h-8 w-full rounded-control border border-line bg-black/60 pl-8 pr-2.5 text-xs text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-link"
          />
        </div>
        <Select
          className="w-40"
          value={sort}
          onChange={setSort}
          ariaLabel={t("gallery.sort")}
          options={[
            { value: "recent", label: t("gallery.sort_recent") },
            { value: "name", label: t("gallery.sort_name") },
            { value: "size", label: t("gallery.sort_size") },
          ]}
        />
      </>
    ) : (
      <h2 className="truncate text-sm font-semibold text-ink">
        {view === "games" ? t("nav.games") : t("nav.settings")}
      </h2>
    );

  const headerActions = (
    <>
      {status.running && view === "clips" && (
        <Button variant="primary" onClick={() => void saveNow()} disabled={busy}>
          {t("rec.save_now")}
        </Button>
      )}
      {view === "clips" && (
        <>
          <Button
            variant="secondary"
            onClick={() => setShowDrive(true)}
            title={t("drive.title")}
          >
            <HardDriveDownload size={13} />
            <span className="hidden sm:inline">{t("drive.browse")}</span>
          </Button>
          <Button variant="ghost" onClick={onPurge} title={t("gallery.purge")}>
            <Trash2 size={13} />
          </Button>
        </>
      )}
    </>
  );

  return (
    <div className="relative flex h-screen flex-col overflow-hidden bg-base font-sans text-ink">
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
          <div className="pointer-events-none fixed left-1/3 top-0 h-[220px] w-[560px] -translate-x-1/2 bg-gradient-to-b from-link/10 via-aqua/5 to-transparent blur-3xl" />

          <div className="relative z-10 flex min-h-0 flex-1">
            <IconRail
              brand={<MoonClipLogo size={26} />}
              items={railItems}
              active={view}
              onSelect={(id) => setView(id as View)}
            >
              <span
                title={`${status.running ? t("rec.recording") : t("status.standby")} · ${
                  currentGame ?? t("status.shortcut", { hotkey })
                }`}
                className={
                  "flex h-10 w-10 items-center justify-center rounded-control " +
                  (status.running ? "text-brand-bright" : "text-ok")
                }
              >
                <span className="relative flex h-2.5 w-2.5">
                  {status.running ? (
                    <span className="h-2.5 w-2.5 rounded-full bg-brand" />
                  ) : (
                    <>
                      <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-ok opacity-50" />
                      <span className="relative inline-flex h-2.5 w-2.5 rounded-full bg-ok" />
                    </>
                  )}
                </span>
              </span>
              <button
                onClick={() => void (status.running ? stop() : start())}
                disabled={busy}
                title={status.running ? t("rec.stop") : t("rec.start")}
                className={
                  "flex h-10 w-10 items-center justify-center rounded-control border transition-colors disabled:opacity-50 " +
                  (status.running
                    ? "border-brand/50 bg-brand/15 text-brand-bright hover:bg-brand/25"
                    : "border-line bg-raised text-ink-soft hover:border-line-strong hover:text-ink")
                }
              >
                {status.running ? <Square size={14} /> : <Circle size={14} />}
              </button>
              {!status.running && (
                <button
                  onClick={() => void startScreen()}
                  disabled={screenBusy}
                  title={t("rec.record_screen")}
                  className="flex h-10 w-10 items-center justify-center rounded-control border border-line bg-raised text-ink-muted transition-colors hover:text-ink disabled:opacity-50"
                >
                  <Monitor size={14} />
                </button>
              )}
              <button
                onClick={() => void setLocale(locale.startsWith("es") ? "en" : "es")}
                title={t("lang.label")}
                className="flex h-8 w-10 items-center justify-center rounded-control font-mono text-[10px] font-semibold uppercase text-ink-faint transition-colors hover:bg-raised hover:text-ink"
              >
                {locale.startsWith("es") ? "ES" : "EN"}
              </button>
            </IconRail>

            {contextRail}

            <div className="flex min-w-0 flex-1 flex-col">
              <ShellHeader actions={headerActions}>{header}</ShellHeader>
              {(purgeMsg || engineError || status.engine_error || engineError) && (
                <div className="border-b border-line px-4 py-1.5">
                  {purgeMsg && <p className="text-[11px] text-ink-muted">{purgeMsg}</p>}
                  {engineError && (
                    <p className="break-all font-mono text-[11px] text-brand-bright">{engineError}</p>
                  )}
                  {status.engine_error && (
                    <p className="break-all font-mono text-[11px] text-warn-bright">
                      {status.engine_error}
                    </p>
                  )}
                </div>
              )}
              <ShellMain className="p-4">
                {view === "settings" && (
                  <SettingsModal
                    engineStatus={status}
                    onHotkeyChange={setHotkey}
                    onOpenWizard={() => setShowWizard(true)}
                  />
                )}
                {view === "games" && <AppManager />}
                {view === "clips" && (
                  <GalleryView
                    refreshToken={galleryTick}
                    onAdvancedEdit={(c) => void openAdvancedEditor(c)}
                    group={group}
                    query={query}
                    sort={sort}
                  />
                )}
              </ShellMain>
            </div>
          </div>
        </div>
      )}
      {showWizard && <SetupWizard onClose={() => setShowWizard(false)} />}
      {showDrive && (
        <DriveBrowser
          onClose={() => setShowDrive(false)}
          onDownloaded={() => onClipSaved()}
        />
      )}
    </div>
  );
}
