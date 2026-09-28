import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ChevronLeft, Download, Folder, Loader2, Video, X } from "lucide-react";
import { Modal } from "../Modal";

interface DriveEntry {
  id: string;
  name: string;
  is_folder: boolean;
  size: number | null;
  modified_time: string | null;
}

interface Progress {
  fileId: string;
  sent: number;
  total: number;
}

function fmtSize(bytes: number | null) {
  if (bytes === null) return "";
  if (bytes >= 1_000_000_000) return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
  if (bytes >= 1_000_000) return `${(bytes / 1_000_000).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(bytes / 1000))} KB`;
}

/** Browse the app's Drive folders and download clips back into the local
 *  game folder (indexed like any other clip). */
export function DriveBrowser({
  onClose,
  onDownloaded,
}: {
  onClose: () => void;
  onDownloaded: () => void;
}) {
  const { t } = useTranslation();
  const [entries, setEntries] = useState<DriveEntry[]>([]);
  const [path, setPath] = useState<{ id: string; name: string }[]>([]);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [downloading, setDownloading] = useState<string | null>(null);
  const [progress, setProgress] = useState<Progress | null>(null);

  const load = async (folderId: string | null) => {
    setBusy(true);
    setError(null);
    try {
      setEntries(await invoke<DriveEntry[]>("drive_browse", { folderId }));
    } catch (e) {
      setError(String(e));
      setEntries([]);
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    void load(null);
  }, []);

  useEffect(() => {
    const unlisten = listen<Progress>("moonclip://download-progress", (event) => {
      setProgress(event.payload);
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  const openFolder = (entry: DriveEntry) => {
    setPath((p) => [...p, { id: entry.id, name: entry.name }]);
    void load(entry.id);
  };

  const goBack = () => {
    const next = path.slice(0, -1);
    setPath(next);
    void load(next.length ? next[next.length - 1].id : null);
  };

  const download = async (entry: DriveEntry) => {
    setDownloading(entry.id);
    setProgress(null);
    setError(null);
    // Remote folders mirror local game folders: the current folder name (or
    // Unknown at the root) is where the clip belongs locally.
    const folder = path.length ? path[path.length - 1].name : "Unknown";
    try {
      await invoke("drive_download", { fileId: entry.id, folder });
      onDownloaded();
    } catch (e) {
      setError(String(e));
    } finally {
      setDownloading(null);
      setProgress(null);
    }
  };

  const pct =
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.sent / progress.total) * 100))
      : null;

  return (
    <Modal>
      <div className="fixed inset-0 z-50 flex items-center justify-center bg-void/80 p-4">
        <div className="max-h-[calc(100vh-2rem)] w-full max-w-lg overflow-y-auto rounded-card border border-line bg-panel p-4 shadow-panel">
        <div className="mb-2 flex items-center gap-2">
          <h3 className="text-sm font-semibold text-ink">{t("drive.title")}</h3>
          <button
            onClick={onClose}
            className="ml-auto rounded-control p-1.5 text-ink-faint transition hover:bg-raised hover:text-ink"
          >
            <X size={15} />
          </button>
        </div>

        <div className="mb-2 flex items-center gap-1 text-xs text-ink-faint">
          <button
            onClick={goBack}
            disabled={path.length === 0}
            className="rounded p-1 transition hover:bg-raised hover:text-ink disabled:opacity-30"
          >
            <ChevronLeft size={14} />
          </button>
          <span className="truncate">
            {t("drive.root")}
            {path.map((p) => ` / ${p.name}`)}
          </span>
        </div>

        <div className="max-h-[50vh] space-y-1 overflow-y-auto pr-1">
          {busy && (
            <p className="flex items-center gap-2 py-4 text-xs text-ink-faint">
              <Loader2 size={13} className="animate-spin" /> {t("drive.loading")}
            </p>
          )}
          {!busy && entries.length === 0 && !error && (
            <p className="py-4 text-xs text-ink-faint">{t("drive.empty")}</p>
          )}
          {entries.map((entry) => (
            <div
              key={entry.id}
              className="flex items-center gap-2 rounded-control border border-line bg-void/50 px-3 py-1.5 text-xs text-ink"
            >
              {entry.is_folder ? (
                <button
                  onClick={() => openFolder(entry)}
                  className="flex min-w-0 flex-1 items-center gap-2 text-left transition hover:text-ink"
                >
                  <Folder size={13} className="shrink-0 text-sky-bright" />
                  <span className="truncate">{entry.name}</span>
                </button>
              ) : (
                <>
                  <Video size={13} className="shrink-0 text-ink-faint" />
                  <span className="min-w-0 flex-1 truncate" title={entry.name}>
                    {entry.name}
                  </span>
                  <span className="shrink-0 font-mono text-[10px] text-ink-faint">
                    {fmtSize(entry.size)}
                  </span>
                  <button
                    onClick={() => void download(entry)}
                    disabled={downloading !== null}
                    className="shrink-0 rounded-control border border-line bg-raised/60 p-1 text-ink-soft transition hover:border-gold/50 hover:text-ink disabled:opacity-50"
                    title={t("drive.download")}
                  >
                    {downloading === entry.id ? (
                      <Loader2 size={13} className="animate-spin" />
                    ) : (
                      <Download size={13} />
                    )}
                  </button>
                </>
              )}
            </div>
          ))}
        </div>

        {downloading && (
          <div className="mt-2">
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-raised">
              <div
                className="h-full rounded-full bg-gold transition-all"
                style={{ width: `${pct ?? 5}%` }}
              />
            </div>
            <p className="mt-1 text-right font-mono text-[10px] text-ink-faint">
              {pct !== null ? `${pct}%` : t("drive.downloading")}
            </p>
          </div>
        )}
        {error && <p className="mt-2 break-all font-mono text-xs text-blood-bright">{error}</p>}
      </div>
    </div>
    </Modal>
  );
}
