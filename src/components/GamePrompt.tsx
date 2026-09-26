import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Gamepad2 } from "lucide-react";
import { registerInputFromCandidate } from "../lib/detect";
import { useCurrentGame } from "../hooks/useCurrentGame";
import { useCustomApps } from "../hooks/useCustomApps";

/**
 * One-time registration prompt: an unknown game is detected while the user
 * plays. Adding it stores the rule + window/token and auto-records forever.
 * Ignoring is remembered per game key.
 */
export function GamePrompt() {
  const { t } = useTranslation();
  const game = useCurrentGame();
  const { registerApp } = useCustomApps();
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState(false);
  const [hidden, setHidden] = useState(false);

  if (!game || game.source !== "fallback" || game.registered || hidden) return null;
  const ignored =
    typeof localStorage !== "undefined" &&
    localStorage.getItem(`moonclip.ignored.${game.game_key}`) === "1";
  if (ignored) return null;

  const add = async () => {
    setBusy(true);
    try {
      await registerApp(registerInputFromCandidate(game));
      setDone(true);
    } finally {
      setBusy(false);
    }
  };

  const ignore = () => {
    localStorage.setItem(`moonclip.ignored.${game.game_key}`, "1");
    setHidden(true);
  };

  return (
    <div className="mb-3 flex flex-wrap items-center gap-2 rounded-xl border border-amber-400/20 bg-amber-400/5 px-3 py-2 text-xs text-amber-100">
      <Gamepad2 size={14} className="shrink-0 text-amber-300" />
      <span className="font-medium">{t("game.detected", { name: game.title })}</span>
      <span className="hidden text-amber-200/70 sm:inline">{t("game.add_hint")}</span>
      <div className="ml-auto flex items-center gap-2">
        {done ? (
          <span className="text-cyan-300">{t("game.registered")}</span>
        ) : (
          <button
            onClick={() => void add()}
            disabled={busy}
            className="rounded-lg border border-amber-400/30 bg-amber-400/10 px-2.5 py-1 font-medium text-amber-100 transition hover:bg-amber-400/20 disabled:opacity-50"
          >
            {t("game.add_and_record")}
          </button>
        )}
        <button
          onClick={ignore}
          className="rounded-lg px-2 py-1 text-amber-200/70 transition hover:bg-white/5 hover:text-amber-100"
        >
          {t("game.ignore")}
        </button>
      </div>
    </div>
  );
}
