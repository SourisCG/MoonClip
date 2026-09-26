import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Gamepad2, Loader2, Pencil, Plus, Trash2 } from "lucide-react";
import { useRegisteredInputs } from "../../hooks/useRegisteredInputs";

/** Games: one button opens the system picker; each row can be re-picked or deleted. */
export function AppManager() {
  const { t } = useTranslation();
  const { inputs, loading, error, register, edit, remove } = useRegisteredInputs();
  const [busy, setBusy] = useState<"register" | string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);

  const runRegister = async () => {
    setFormError(null);
    setBusy("register");
    try {
      await register();
    } catch (e) {
      setFormError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const runEdit = async (id: string) => {
    setFormError(null);
    setBusy(id);
    try {
      await edit(id);
    } catch (e) {
      setFormError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const runDelete = async (id: string) => {
    setFormError(null);
    try {
      await remove(id);
    } catch (e) {
      setFormError(String(e));
    }
  };

  if (loading) return <p className="text-sm text-slate-400">{t("common.loading")}</p>;
  if (error) return <p className="text-sm text-red-400">{error}</p>;

  return (
    <div className="max-w-2xl space-y-4">
      <button
        onClick={() => void runRegister()}
        disabled={busy !== null}
        className="inline-flex w-full items-center justify-center gap-2 rounded-xl border border-cyan-500/40 bg-cyan-500/15 px-4 py-2.5 text-sm font-semibold uppercase tracking-wide text-cyan-100 transition hover:bg-cyan-500/25 disabled:cursor-wait disabled:opacity-60 sm:w-auto"
      >
        {busy === "register" ? <Loader2 size={16} className="animate-spin" /> : <Plus size={16} />}
        {t("games.register_game")}
      </button>
      <p className="text-xs text-slate-500">{t("games.register_hint")}</p>

      {busy !== null && (
        <div className="flex items-center gap-2 rounded-xl border border-cyan-500/30 bg-cyan-500/10 px-3 py-2.5 text-sm text-cyan-100">
          <Loader2 size={15} className="shrink-0 animate-spin" />
          <span>{t("games.picking")}</span>
        </div>
      )}
      {formError && <p className="break-words font-mono text-xs text-red-400">{formError}</p>}

      <div>
        <h4 className="mb-2 text-sm font-semibold text-slate-200">
          {t("games.registered_title")}
        </h4>
        {inputs.length === 0 ? (
          <p className="text-sm text-slate-500">{t("games.empty")}</p>
        ) : (
          <ul className="space-y-2">
            {inputs.map((g) => (
              <li
                key={g.id}
                className="flex flex-col items-start gap-2 rounded-xl border border-white/5 bg-black/30 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between sm:gap-3 sm:px-4"
              >
                <div className="flex w-full min-w-0 items-center gap-2.5 sm:w-auto">
                  <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-white/10 text-cyan-200">
                    <Gamepad2 size={15} />
                  </span>
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-slate-200">
                      {g.display_name}
                    </p>
                    {!g.window_title && (
                      <p className="text-[11px] text-amber-300/80">
                        {t("games.no_identity")}
                      </p>
                    )}
                  </div>
                </div>
                <div className="flex w-full items-center gap-1.5 sm:w-auto">
                  <button
                    onClick={() => void runEdit(g.id)}
                    disabled={busy !== null}
                    title={t("games.edit_hint")}
                    className="inline-flex flex-1 items-center justify-center gap-1.5 rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-slate-200 transition hover:bg-white/10 disabled:cursor-wait disabled:opacity-50 sm:flex-none"
                  >
                    {busy === g.id ? (
                      <Loader2 size={13} className="animate-spin" />
                    ) : (
                      <Pencil size={13} />
                    )}
                    {t("games.edit")}
                  </button>
                  <button
                    onClick={() => void runDelete(g.id)}
                    disabled={busy !== null}
                    title={t("games.delete")}
                    className="rounded-lg p-2 text-slate-500 transition hover:bg-red-500/20 hover:text-red-300 disabled:opacity-50"
                  >
                    <Trash2 size={15} />
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
