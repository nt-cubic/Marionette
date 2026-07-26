import { useState } from "react";
import { saveProviderKey } from "../lib/api";

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
  const [provider, setProvider] = useState(PROVIDERS[0].id);
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

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
      onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="project-dialog-backdrop" onClick={onClose}>
      <div
        className="project-dialog provider-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="project-dialog__header">添加 API Key</div>
        <div className="project-dialog__body">
          <label className="provider-dialog__field">
            <span className="provider-dialog__label">服务商</span>
            <select
              className="provider-dialog__select"
              value={provider}
              onChange={(e) => setProvider(e.target.value)}
              disabled={saving}
            >
              {PROVIDERS.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.label}
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
              placeholder="粘贴你的 API Key"
              disabled={saving}
              autoFocus
            />
          </label>
          {error && <div className="provider-dialog__error">{error}</div>}
        </div>
        <div className="project-dialog__actions">
          <button
            className="project-dialog__cancel"
            type="button"
            onClick={onClose}
            disabled={saving}
          >
            取消
          </button>
          <button
            className="project-dialog__submit"
            type="button"
            onClick={handleSave}
            disabled={saving || !apiKey.trim()}
          >
            {saving ? "保存中…" : "保存并连接"}
          </button>
        </div>
      </div>
    </div>
  );
}
