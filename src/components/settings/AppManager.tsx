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

  if (loading) return <p className="text-sm text-ink-muted">{t("common.loading")}</p>;
  if (error) return <p className="text-sm text-blood-bright">{error}</p>;

  return (
    <div className="max-w-2xl space-y-4">
      <button
        onClick={() => void runRegister()}
        disabled={busy !== null}
        className="inline-flex w-full items-center justify-center gap-2 rounded-control border border-line bg-raised px-4 py-2.5 text-sm font-semibold uppercase tracking-wide text-ink transition hover:bg-link/25 disabled:cursor-wait disabled:opacity-60 sm:w-auto"
      >
        {busy === "register" ? <Loader2 size={16} className="animate-spin" /> : <Plus size={16} />}
        {t("games.register_game")}
      </button>
      <p className="text-xs text-ink-faint">{t("games.register_hint")}</p>

      {busy !== null && (
        <div className="flex items-center gap-2 rounded-xl border border-gold/40 bg-sky/10 px-3 py-2.5 text-sm text-ink">
          <Loader2 size={15} className="shrink-0 animate-spin" />
          <span>{t("games.picking")}</span>
        </div>
      )}
      {formError && <p className="break-words font-mono text-xs text-blood-bright">{formError}</p>}

      <div>
        <h4 className="mb-2 text-sm font-semibold text-ink">
          {t("games.registered_title")}
        </h4>
        {inputs.length === 0 ? (
          <p className="text-sm text-ink-faint">{t("games.empty")}</p>
        ) : (
          <ul className="space-y-2">
            {inputs.map((g) => (
              <li
                key={g.id}
                className="flex flex-col items-start gap-2 rounded-xl border border-line bg-void/50 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between sm:gap-3 sm:px-4"
              >
                <div className="flex w-full min-w-0 items-center gap-2.5 sm:w-auto">
                  <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-control bg-raised text-ink">
                    <Gamepad2 size={15} />
                  </span>
                  <div className="min-w-0">
                    <p className="truncate text-sm font-medium text-ink">
                      {g.display_name}
                    </p>
                    {!g.window_title && (
                      <p className="text-[11px] text-gold-bright/80">
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
                    className="inline-flex flex-1 items-center justify-center gap-1.5 rounded-control border border-line bg-raised/60 px-3 py-1.5 text-xs text-ink transition hover:bg-raised disabled:cursor-wait disabled:opacity-50 sm:flex-none"
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
                    className="rounded-control p-2 text-ink-faint transition hover:bg-blood/20 hover:text-blood-bright disabled:opacity-50"
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
