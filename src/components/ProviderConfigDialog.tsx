import { useCallback, useEffect, useState } from "react";
import { deleteProviderKey, listProviders, saveProviderKey } from "../lib/api";
import type { ProviderInfo } from "../lib/types";

const PROVIDERS = [
  { id: "deepseek", label: "DeepSeek" },
  { id: "openrouter", label: "OpenRouter" },
  { id: "openai", label: "OpenAI" },
  { id: "anthropic", label: "Anthropic" },
  { id: "google", label: "Google" },
  { id: "xai", label: "xAI (Grok)" },
  { id: "zai", label: "Z.AI (GLM)" },
  { id: "siliconflow", label: "SiliconFlow" },
];

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
  const [configuredProviders, setConfiguredProviders] = useState<ProviderInfo[]>([]);
  const [provider, setProvider] = useState(PROVIDERS[0].id);
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [confirming, setConfirming] = useState<Confirming | null>(null);

  const loadProviders = useCallback(async () => {
    try {
      const list = await listProviders();
      setConfiguredProviders(list);
    } catch {
      // Silently fail — the add form still works.
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void loadProviders();
  }, [loadProviders]);

  const selected = configuredProviders.find((p) => p.provider === provider);
  const isConfigured = selected != null;
  // Overwriting an OAuth login destroys a refresh token this app cannot mint
  // again, so it takes a deliberate second click and an explicit `force`.
  const selectedIsOauth = selected?.authKind === "oauth";
  const saveConfirmed =
    confirming?.action === "save" && confirming.provider === provider;

  const handleSave = async () => {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      setError("请输入 API Key");
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
      setConfiguredProviders((prev) => prev.filter((p) => p.provider !== target.provider));
      setConfirming(null);
      // If the user was editing this provider, drop the half-typed key.
      if (provider === target.provider) setApiKey("");
      onKeysChanged();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  };

  const configuredIds = new Set(configuredProviders.map((p) => p.provider));
  const busy = saving || deleting !== null;

  return (
    <div className="project-dialog-backdrop" onClick={busy ? undefined : onClose}>
      <div
        className="project-dialog provider-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="project-dialog__header">管理 API Key</div>
        <div className="project-dialog__body">
          {/* ── List of configured providers ── */}
          {loaded && configuredProviders.length > 0 && (
            <div className="provider-dialog__section">
              <span className="provider-dialog__section-title">已配置的 Key</span>
              <div className="provider-dialog__list">
                {configuredProviders.map((p) => {
                  const pendingDelete =
                    confirming?.action === "delete" && confirming.provider === p.provider;
                  const isOauth = p.authKind === "oauth";
                  return (
                    <div key={p.provider} className="provider-dialog__item">
                      <span className="provider-dialog__item-name">
                        <span className="provider-dialog__item-dot" />
                        {p.label}
                        {isOauth && (
                          <span
                            className="provider-dialog__item-badge"
                            title="通过 opencode auth login 登录，删除后需要重新登录"
                          >
                            OAuth
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
                            ? `删除 ${p.label} 的 OAuth 登录（需重新 opencode auth login）`
                            : `删除 ${p.label} 的 API Key`
                        }
                      >
                        {deleting === p.provider
                          ? "删除中…"
                          : pendingDelete
                            ? isOauth
                              ? "确认删除登录"
                              : "确认删除"
                            : "删除"}
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {loaded && configuredProviders.length === 0 && (
            <div className="provider-dialog__empty">
              还没有配置任何 API Key
            </div>
          )}

          {/* ── Add / Edit form ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">
              {isConfigured ? "更新 Key" : "添加新的 Key"}
            </span>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">服务商</span>
              <select
                className="provider-dialog__select"
                value={provider}
                onChange={(e) => {
                  setProvider(e.target.value);
                  setConfirming(null);
                  setError("");
                }}
                disabled={saving}
              >
                {PROVIDERS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                    {configuredIds.has(p.id) ? " (已配置)" : ""}
                  </option>
                ))}
              </select>
            </label>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">API Key</span>
              <input
                className="provider-dialog__input"
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder={isConfigured ? "输入新 Key 覆盖已有配置" : "粘贴你的 API Key"}
                disabled={saving}
                autoFocus
              />
            </label>
            {selectedIsOauth && (
              <div className="provider-dialog__warning">
                {selected?.label} 目前是 OAuth 登录。写入 API Key 会覆盖登录凭证，
                之后只能用 <code>opencode auth login</code> 重新登录。
              </div>
            )}
            {error && <div className="provider-dialog__error">{error}</div>}
          </div>

          {restartPending && (
            <div className="provider-dialog__note">
              关闭后会重启 agent，让它重新读取 auth.json。
            </div>
          )}
        </div>
        <div className="project-dialog__actions">
          <button
            className="project-dialog__cancel"
            type="button"
            onClick={onClose}
            disabled={busy}
          >
            关闭
          </button>
          <button
            className="project-dialog__submit"
            type="button"
            onClick={handleSave}
            disabled={saving || !apiKey.trim()}
          >
            {saving
              ? "保存中…"
              : selectedIsOauth
                ? saveConfirmed
                  ? "确认覆盖 OAuth 登录"
                  : "覆盖 OAuth 登录…"
                : isConfigured
                  ? "更新 Key"
                  : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
