import { useCallback, useEffect, useMemo, useState } from "react";
import {
  deleteProviderKey,
  listProviders,
  saveProviderKey,
  upsertProviderMeta,
} from "../lib/api";
import type { ProviderInfo } from "../lib/types";
import { useT } from "../lib/i18n";
import { DialogShell } from "./DialogShell";

type Props = {
  onClose: () => void;
  /**
   * A key was added or removed. The dialog stays open — managing several keys in
   * one visit is the point — so the parent should only note that auth changed
   * and act on it when the dialog closes.
   */
  onKeysChanged: () => void;
  /** True once `onKeysChanged` fired, so the footer can explain the restart. */
  restartPending?: boolean;
};

/** A pending destructive action awaiting a second click. */
type Confirming = { action: "save" | "delete"; provider: string };

export function ProviderConfigDialog({ onClose, onKeysChanged, restartPending }: Props) {
  const [catalog, setCatalog] = useState<ProviderInfo[]>([]);
  const [provider, setProvider] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [customId, setCustomId] = useState("");
  const [customLabel, setCustomLabel] = useState("");
  const [showCustom, setShowCustom] = useState(false);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [confirming, setConfirming] = useState<Confirming | null>(null);
  const t = useT();

  const loadProviders = useCallback(async () => {
    try {
      const list = await listProviders();
      setCatalog(list);
      setProvider((current) => {
        if (current && list.some((p) => p.provider === current)) return current;
        return list[0]?.provider ?? "";
      });
    } catch (err) {
      // 目录加载失败也要提示；添加表单仍然可用。
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void loadProviders();
  }, [loadProviders]);

  const selected = catalog.find((p) => p.provider === provider);
  const isConfigured = selected?.configured === true || selected?.hasKey === true;
  const selectedIsOauth = selected?.authKind === "oauth";
  const saveConfirmed =
    confirming?.action === "save" && confirming.provider === provider;
  const probeUnsupported =
    !selected?.probeStrategy || selected.probeStrategy === "none";

  const configuredList = useMemo(
    () => catalog.filter((p) => p.configured || p.hasKey),
    [catalog],
  );

  const handleSave = async () => {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      setError(t("creds.needApiKey"));
      return;
    }
    if (!provider) {
      setError(t("provider.needProvider"));
      return;
    }
    if (selectedIsOauth && !saveConfirmed) {
      setConfirming({ action: "save", provider });
      setError("");
      return;
    }
    setSaving(true);
    setError("");
    try {
      await saveProviderKey(provider, trimmed, selectedIsOauth);
      setApiKey("");
      setConfirming(null);
      await loadProviders();
      onKeysChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleAddCustom = async () => {
    const id = customId.trim().toLowerCase();
    const label = customLabel.trim() || id;
    if (!id) {
      setError(t("provider.needCustomId"));
      return;
    }
    if (!/^[a-z0-9][a-z0-9_-]*$/.test(id)) {
      setError(t("provider.invalidId"));
      return;
    }
    setSaving(true);
    setError("");
    try {
      await upsertProviderMeta(id, label, [id], "none");
      if (apiKey.trim()) {
        await saveProviderKey(id, apiKey.trim(), false);
        onKeysChanged();
      }
      setCustomId("");
      setCustomLabel("");
      setApiKey("");
      setShowCustom(false);
      await loadProviders();
      setProvider(id);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (target: ProviderInfo) => {
    const confirmed =
      confirming?.action === "delete" && confirming.provider === target.provider;
    if (!confirmed) {
      setConfirming({ action: "delete", provider: target.provider });
      setError("");
      return;
    }
    setDeleting(target.provider);
    setError("");
    try {
      await deleteProviderKey(target.provider, target.authKind === "oauth");
      setConfirming(null);
      if (provider === target.provider) setApiKey("");
      await loadProviders();
      onKeysChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  };

  const busy = saving || deleting !== null;

  return (
    <DialogShell
      className="project-dialog-backdrop"
      dialogClassName="project-dialog provider-dialog"
      label={t("provider.dialogLabel")}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
        <div className="project-dialog__header">{t("provider.dialogLabel")}</div>
        <div className="project-dialog__body">
          <p className="provider-dialog__lead">
            {t("provider.leadBefore")}<code>auth.json</code>{t("provider.leadAfter")}
          </p>

          {/* ── List of configured providers ── */}
          {loaded && configuredList.length > 0 && (
            <div className="provider-dialog__section">
              <span className="provider-dialog__section-title">{t("provider.configuredKeys")}</span>
              <div className="provider-dialog__list">
                {configuredList.map((p) => {
                  const pendingDelete =
                    confirming?.action === "delete" && confirming.provider === p.provider;
                  const isOauth = p.authKind === "oauth";
                  const noProbe = !p.probeStrategy || p.probeStrategy === "none";
                  return (
                    <div key={p.provider} className="provider-dialog__item">
                      <span className="provider-dialog__item-name">
                        <span className="provider-dialog__item-dot" />
                        {p.label}
                        <span className="provider-dialog__item-id">{p.provider}</span>
                        {isOauth && (
                          <span
                            className="provider-dialog__item-badge"
                            title={t("provider.oauthBadgeTitle")}
                          >
                            OAuth
                          </span>
                        )}
                        {noProbe && (
                          <span
                            className="provider-dialog__item-badge provider-dialog__item-badge--muted"
                            title={t("provider.noProbeTitle")}
                          >
                            {t("provider.noProbeBadge")}
                          </span>
                        )}
                      </span>
                      <button
                        className={
                          pendingDelete
                            ? "provider-dialog__delete-btn is-confirming"
                            : "provider-dialog__delete-btn"
                        }
                        type="button"
                        onClick={() => void handleDelete(p)}
                        disabled={deleting === p.provider}
                        title={
                          isOauth
                            ? t("provider.deleteOauthTitle", { name: p.label })
                            : t("provider.deleteKeyTitle", { name: p.label })
                        }
                      >
                        {deleting === p.provider
                          ? t("creds.deleting")
                          : pendingDelete
                            ? isOauth
                              ? t("provider.confirmDeleteLogin")
                              : t("creds.confirmDelete")
                            : t("common.delete")}
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {loaded && configuredList.length === 0 && (
            <div className="provider-dialog__empty">{t("provider.noKeys")}</div>
          )}

          {/* ── Add / Edit form ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">
              {isConfigured ? t("provider.updateKey") : t("provider.addKey")}
            </span>
            {!showCustom ? (
              <>
                <label className="provider-dialog__field">
                  <span className="provider-dialog__label">{t("creds.provider")}</span>
                  <select
                    className="provider-dialog__select"
                    value={provider}
                    onChange={(e) => {
                      setProvider(e.target.value);
                      setConfirming(null);
                      setError("");
                    }}
                    disabled={saving || catalog.length === 0}
                  >
                    {catalog.map((p) => (
                      <option key={p.provider} value={p.provider}>
                        {p.label}
                        {p.configured || p.hasKey ? t("provider.configuredSuffix") : ""}
                        {p.probeStrategy === "none" || !p.probeStrategy ? t("provider.noProbeSuffix") : ""}
                      </option>
                    ))}
                  </select>
                </label>
                <button
                  type="button"
                  className="provider-dialog__link"
                  onClick={() => {
                    setShowCustom(true);
                    setError("");
                  }}
                >
                  {t("provider.addCustom")}
                </button>
              </>
            ) : (
              <>
                <label className="provider-dialog__field">
                  <span className="provider-dialog__label">Provider id</span>
                  <input
                    className="provider-dialog__input"
                    value={customId}
                    onChange={(e) => setCustomId(e.target.value)}
                    placeholder="moonshot"
                    disabled={saving}
                  />
                </label>
                <label className="provider-dialog__field">
                  <span className="provider-dialog__label">{t("provider.displayName")}</span>
                  <input
                    className="provider-dialog__input"
                    value={customLabel}
                    onChange={(e) => setCustomLabel(e.target.value)}
                    placeholder="Moonshot"
                    disabled={saving}
                  />
                </label>
                <p className="provider-dialog__hint">
                  {t("provider.customHint")}
                </p>
                <button
                  type="button"
                  className="provider-dialog__link"
                  onClick={() => setShowCustom(false)}
                >
                  {t("provider.backToCatalog")}
                </button>
              </>
            )}
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">API Key</span>
              <input
                className="provider-dialog__input"
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder={isConfigured ? t("provider.keyOverwritePlaceholder") : t("provider.keyPlaceholder")}
                disabled={saving}
                autoFocus
              />
            </label>
            {!showCustom && selected && probeUnsupported && (
              <div className="provider-dialog__hint">
                {t("provider.probeHintPre")}<strong>{t("provider.probeHintStrong")}</strong>{t("provider.probeHintPost")}
              </div>
            )}
            {selectedIsOauth && (
              <div className="provider-dialog__warning">
                {t("provider.oauthWarningPre", { name: selected?.label ?? "" })}
                <code>opencode auth login</code>{t("provider.oauthWarningPost")}
              </div>
            )}
            {error && <div className="provider-dialog__error">{error}</div>}
          </div>

          {restartPending && (
            <div className="provider-dialog__note">
              {t("provider.restartNote")}
            </div>
          )}
        </div>
        <div className="project-dialog__actions">
          <button type="button" className="project-dialog__cancel" onClick={onClose} disabled={busy}>
            {t("common.close")}
          </button>
          {showCustom ? (
            <button
              type="button"
              className="project-dialog__submit"
              disabled={busy}
              onClick={() => void handleAddCustom()}
            >
              {saving ? t("creds.saving") : t("provider.addProvider")}
            </button>
          ) : (
            <button
              type="button"
              className="project-dialog__submit"
              disabled={busy || !provider}
              onClick={() => void handleSave()}
            >
              {saving
                ? t("creds.saving")
                : saveConfirmed
                  ? t("provider.confirmOverwriteOauth")
                  : isConfigured
                    ? t("provider.updateKey")
                    : t("provider.saveKey")}
            </button>
          )}
        </div>
    </DialogShell>
  );
}
