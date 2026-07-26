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
  onSaved: () => void;
};

export function ProviderConfigDialog({ onClose, onSaved }: Props) {
  const [configuredProviders, setConfiguredProviders] = useState<ProviderInfo[]>([]);
  const [provider, setProvider] = useState(PROVIDERS[0].id);
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [loaded, setLoaded] = useState(false);

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

  const isConfigured = configuredProviders.some((p) => p.provider === provider);

  const handleSave = async () => {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      setError("请输入 API Key");
      return;
    }
    setSaving(true);
    setError("");
    try {
      await saveProviderKey(provider, trimmed);
      setApiKey("");
      // Refresh list and notify parent
      await loadProviders();
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (providerId: string) => {
    setDeleting(providerId);
    setError("");
    try {
      await deleteProviderKey(providerId);
      setConfiguredProviders((prev) => prev.filter((p) => p.provider !== providerId));
      // If the user was editing this provider, reset the form
      if (provider === providerId) {
        setApiKey("");
      }
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  };

  const configuredIds = new Set(configuredProviders.map((p) => p.provider));

  return (
    <div className="project-dialog-backdrop" onClick={onClose}>
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
                {configuredProviders.map((p) => (
                  <div key={p.provider} className="provider-dialog__item">
                    <span className="provider-dialog__item-name">
                      <span className="provider-dialog__item-dot" />
                      {p.label}
                    </span>
                    <button
                      className="provider-dialog__delete-btn"
                      type="button"
                      onClick={() => void handleDelete(p.provider)}
                      disabled={deleting === p.provider}
                      title={`删除 ${p.label} 的 API Key`}
                    >
                      {deleting === p.provider ? "删除中…" : "删除"}
                    </button>
                  </div>
                ))}
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
            {error && <div className="provider-dialog__error">{error}</div>}
          </div>
        </div>
        <div className="project-dialog__actions">
          <button
            className="project-dialog__cancel"
            type="button"
            onClick={onClose}
            disabled={saving || deleting !== null}
          >
            关闭
          </button>
          <button
            className="project-dialog__submit"
            type="button"
            onClick={handleSave}
            disabled={saving || !apiKey.trim()}
          >
            {saving ? "保存中…" : isConfigured ? "更新 Key" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
