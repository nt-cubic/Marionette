import { useEffect, useId, useState } from "react";
import {
  CHEAP_TEXT_MAX_TIMEOUT_MS,
  CHEAP_TEXT_MAX_TOKENS,
  CHEAP_TEXT_MIN_MAX_TOKENS,
  CHEAP_TEXT_MIN_TIMEOUT_MS,
  CHEAP_TEXT_TITLE_STYLE_LIMIT,
  cheapTextConfigured,
  cheapTextViewToDraft,
  getCheapTextSettings,
  listCheapTextSources,
  saveCheapTextSettings,
  type CheapTextSettings,
  type CheapTextSettingsDraft,
  type CheapTextSource,
} from "../lib/cheapText";
import { useT } from "../lib/i18n";
import { DialogShell } from "./DialogShell";

/** One source row; the id is appended only when it adds information. */
function sourceLabel(source: CheapTextSource): string {
  return source.label === source.id ? source.id : `${source.label} (${source.id})`;
}

type BodyProps = {
  onSaved?: (settings: CheapTextSettings) => void;
  /** Present in the dialog variant; omitted when embedded as a panel. */
  onClose?: () => void;
};

/**
 * Settings body for the cheap-text model (P1-13).
 *
 * Compact on purpose: one source, one model, optional title style, token and
 * timeout caps. Credential readiness is a read-only hint from the same source
 * store the engine uses; this dialog never asks for a key and never calls the
 * network. Saving keeps the independent usage source intact (`cheap-text`).
 */
function CheapTextSettingsBody({ onSaved, onClose }: BodyProps) {
  const t = useT();
  const [draft, setDraft] = useState<CheapTextSettingsDraft>(() =>
    cheapTextViewToDraft(null),
  );
  const [saved, setSaved] = useState<CheapTextSettings | null>(null);
  const [sources, setSources] = useState<CheapTextSource[]>([]);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const [savedNotice, setSavedNotice] = useState(false);

  useEffect(() => {
    let live = true;
    void (async () => {
      const [view, catalog] = await Promise.all([
        getCheapTextSettings(),
        listCheapTextSources(),
      ]);
      if (!live) return;
      setSaved(view);
      setDraft(cheapTextViewToDraft(view));
      setSources(catalog);
      setLoading(false);
    })();
    return () => {
      live = false;
    };
  }, []);

  const patch = (next: Partial<CheapTextSettingsDraft>) => {
    setSavedNotice(false);
    setError("");
    setDraft((previous) => ({ ...previous, ...next }));
  };

  const selectedSource = sources.find(
    (source) => source.id === draft.source || source.label === draft.source,
  );
  const models = selectedSource?.models ?? [];
  const configuredNow = cheapTextConfigured(draft);
  const protocolUsable = !selectedSource || selectedSource.protocol !== "none";
  const keyReady = selectedSource
    ? selectedSource.hasKey
    : Boolean(
        saved?.credentialReady &&
          saved.source.trim().toLowerCase() === draft.source.trim().toLowerCase(),
      );
  const ready = draft.enabled && configuredNow && protocolUsable && keyReady;
  const readinessText = !draft.enabled
    ? t("cheapText.readinessOff")
    : !configuredNow
      ? t("cheapText.readinessUnset")
      : !protocolUsable
        ? t("cheapText.readinessProtocol")
        : keyReady
          ? t("cheapText.readinessReady")
          : t("cheapText.readinessMissing");
  const savedText = !saved
    ? ""
    : !saved.configured
      ? t("cheapText.savedUnset")
      : saved.credentialReady
        ? t("cheapText.savedReady")
        : t("cheapText.savedMissing");

  const pickSource = (id: string) => {
    if (!id) {
      patch({ source: "", model: "" });
      return;
    }
    const source = sources.find((row) => row.id === id);
    const keepModel =
      source && source.models.includes(draft.model) ? draft.model : undefined;
    patch({
      source: id,
      model: keepModel ?? source?.models[0] ?? draft.model,
    });
  };

  const save = async () => {
    setSaving(true);
    setError("");
    setSavedNotice(false);
    const result = await saveCheapTextSettings(draft);
    setSaving(false);
    if (!result.ok) {
      setError(result.error || t("cheapText.saveFailed"));
      return;
    }
    setSaved(result.settings);
    setDraft(cheapTextViewToDraft(result.settings));
    setSavedNotice(true);
    onSaved?.(result.settings);
  };

  if (loading) {
    return <p className="cheap-text__hint">{t("cheapText.loading")}</p>;
  }

  return (
    <>
      <div className="cheap-text__status">
        <span
          className={
            ready ? "cheap-text__chip is-ready" : "cheap-text__chip is-warn"
          }
        >
          {readinessText}
        </span>
        {savedText ? <span className="cheap-text__saved">{savedText}</span> : null}
      </div>

      <p className="cheap-text__lead">{t("cheapText.lead")}</p>

      <label className="cheap-text__toggle">
        <input
          type="checkbox"
          checked={draft.enabled}
          disabled={saving}
          onChange={(event) => patch({ enabled: event.target.checked })}
        />
        <span>{t("cheapText.enable")}</span>
      </label>

      <label className="project-dialog__field">
        <span>{t("cheapText.source")}</span>
        {sources.length > 0 ? (
          <select
            value={selectedSource?.id ?? draft.source}
            disabled={saving}
            onChange={(event) => pickSource(event.target.value)}
          >
            <option value="">{t("cheapText.sourcePlaceholder")}</option>
            {draft.source && !selectedSource ? (
              <option value={draft.source}>{draft.source}</option>
            ) : null}
            {sources.map((source) => (
              <option
                key={source.id}
                value={source.id}
                disabled={source.protocol === "none"}
              >
                {sourceLabel(source)}
              </option>
            ))}
          </select>
        ) : (
          <input
            type="text"
            spellCheck={false}
            autoComplete="off"
            value={draft.source}
            disabled={saving}
            placeholder={t("cheapText.sourcePlaceholder")}
            onChange={(event) => patch({ source: event.target.value })}
          />
        )}
      </label>

      <label className="project-dialog__field">
        <span>{t("cheapText.model")}</span>
        {models.length > 0 ? (
          <select
            value={draft.model}
            disabled={saving}
            onChange={(event) => patch({ model: event.target.value })}
          >
            <option value="">{t("cheapText.modelPlaceholder")}</option>
            {draft.model && !models.includes(draft.model) ? (
              <option value={draft.model}>{draft.model}</option>
            ) : null}
            {models.map((model) => (
              <option key={model} value={model}>
                {model}
              </option>
            ))}
          </select>
        ) : (
          <input
            type="text"
            spellCheck={false}
            autoComplete="off"
            value={draft.model}
            disabled={saving}
            placeholder={t("cheapText.modelPlaceholder")}
            onChange={(event) => patch({ model: event.target.value })}
          />
        )}
      </label>

      <label className="project-dialog__field">
        <span>{t("cheapText.style")}</span>
        <input
          type="text"
          spellCheck={false}
          autoComplete="off"
          maxLength={CHEAP_TEXT_TITLE_STYLE_LIMIT}
          value={draft.titleStyle}
          disabled={saving}
          placeholder={t("cheapText.stylePlaceholder")}
          onChange={(event) => patch({ titleStyle: event.target.value })}
        />
      </label>

      <label className="project-dialog__field">
        <span>{t("cheapText.maxTokens")}</span>
        <input
          type="number"
          inputMode="numeric"
          min={CHEAP_TEXT_MIN_MAX_TOKENS}
          max={CHEAP_TEXT_MAX_TOKENS}
          step={8}
          value={draft.maxTokens}
          disabled={saving}
          onChange={(event) =>
            patch({
              maxTokens: event.target.value === "" ? 0 : Number(event.target.value),
            })
          }
        />
      </label>

      <label className="project-dialog__field">
        <span>{t("cheapText.timeout")}</span>
        <input
          type="number"
          inputMode="numeric"
          min={CHEAP_TEXT_MIN_TIMEOUT_MS}
          max={CHEAP_TEXT_MAX_TIMEOUT_MS}
          step={500}
          value={draft.timeoutMs}
          disabled={saving}
          onChange={(event) =>
            patch({
              timeoutMs: event.target.value === "" ? 0 : Number(event.target.value),
            })
          }
        />
      </label>

      <p className="cheap-text__hint">{t("cheapText.limitsHint")}</p>

      {sources.length === 0 ? (
        <p className="cheap-text__hint">{t("cheapText.noSources")}</p>
      ) : null}

      {error ? (
        <p className="project-dialog__error" role="alert">
          {error}
        </p>
      ) : null}
      {savedNotice && !error ? (
        <p className="cheap-text__saved-notice" role="status">
          {t("cheapText.savedStatus")}
        </p>
      ) : null}

      <div className="project-dialog__actions">
        {onClose ? (
          <button
            type="button"
            className="project-dialog__cancel"
            disabled={saving}
            onClick={onClose}
          >
            {t("common.close")}
          </button>
        ) : null}
        <button
          type="button"
          className="project-dialog__submit"
          disabled={saving}
          onClick={() => void save()}
        >
          {saving ? t("cheapText.saving") : t("cheapText.save")}
        </button>
      </div>
    </>
  );
}

/**
 * Modal entry point. Mount it from any settings host; it owns no global state,
 * so it is safe in the main window and detached settings surfaces alike.
 */
export function CheapTextSettingsDialog({
  onClose,
  onSaved,
}: {
  onClose: () => void;
  onSaved?: (settings: CheapTextSettings) => void;
}) {
  const t = useT();
  const titleId = useId();
  return (
    <DialogShell
      className="project-dialog-backdrop"
      dialogClassName="project-dialog cheap-text-dialog"
      labelledBy={titleId}
      onClose={onClose}
    >
      <div className="project-dialog__header">
        <div>
          <strong id={titleId}>{t("cheapText.title")}</strong>
        </div>
        <button
          type="button"
          className="project-dialog__close"
          aria-label={t("common.close")}
          onClick={onClose}
        >
          ×
        </button>
      </div>
      <CheapTextSettingsBody onClose={onClose} onSaved={onSaved} />
    </DialogShell>
  );
}

/**
 * Embeddable variant for a settings page that already renders its own chrome.
 * Same load/save/credential logic as the dialog.
 */
export function CheapTextSettingsPanel({
  onSaved,
  title,
}: {
  onSaved?: (settings: CheapTextSettings) => void;
  title?: string;
}) {
  const t = useT();
  return (
    <section
      className="cheap-text-panel"
      aria-label={title ?? t("cheapText.title")}
    >
      {title ? <div className="cheap-text-panel__head">{title}</div> : null}
      <CheapTextSettingsBody onSaved={onSaved} />
    </section>
  );
}
