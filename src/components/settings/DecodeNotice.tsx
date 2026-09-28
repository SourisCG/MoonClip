import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";

interface DecodeStatus {
  vendor: string;
  distro: string;
  hardware: boolean;
  driver: string | null;
  missing_package: string | null;
}

const DOC_URL =
  "https://github.com/SourisCG/MoonClip/blob/main/docs/10_DEPENDENCIES.md";

/**
 * Playback decode line for the Settings modal. Hardware decode is optional:
 * software decoders always ship with the bundled ffmpeg, so a missing VA-API
 * driver is a notice, never a blocker. Only the package NAME is shown (each
 * distro's install command lives in the dependency guide).
 */
export function DecodeNotice() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<DecodeStatus | null>(null);

  useEffect(() => {
    invoke<DecodeStatus>("decode_status")
      .then(setStatus)
      .catch(() => {});
  }, []);

  if (!status) return null;

  const row =
    "flex flex-col items-start gap-1.5 rounded-xl border border-line bg-void/50 px-3 py-3 sm:flex-row sm:items-center sm:justify-between sm:gap-4 sm:px-4";

  if (status.hardware) {
    return (
      <div className={row}>
        <span className="text-sm text-ink-soft">{t("decode.title")}</span>
        <span className="font-mono text-xs text-jade-bright">
          {t("decode.hardware")}
          {status.driver ? ` · ${status.driver}` : ""}
        </span>
      </div>
    );
  }

  return (
    <div className={row}>
      <span className="text-sm text-ink-soft">{t("decode.title")}</span>
      <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
        <span className="font-mono text-xs text-gold-bright">
          {t("decode.software")}
        </span>
        {status.missing_package && (
          <span className="text-xs text-ink-muted">
            {t("decode.install")}{" "}
            <code className="font-mono text-amber-200">
              {status.missing_package}
            </code>
          </span>
        )}
        <button
          onClick={() => void openUrl(DOC_URL).catch(() => {})}
          className="text-xs text-sky-bright underline decoration-link/40 transition hover:text-ink"
        >
          {t("decode.doc")}
        </button>
      </span>
    </div>
  );
}
