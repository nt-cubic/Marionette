import { useCallback, useEffect, useMemo, useState } from "react";
import {
  deleteProviderKey,
  listProviders,
  saveProviderKey,
  upsertProviderMeta,
} from "../lib/api";
import type { ProviderInfo } from "../lib/types";

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

  const loadProviders = useCallback(async () => {
    try {
      const list = await listProviders();
      setCatalog(list);
      setProvider((current) => {
        if (current && list.some((p) => p.provider === current)) return current;
        return list[0]?.provider ?? "";
      });
    } catch {
      // Silently fail — the add form still works.
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
      setError("请输入 API Key");
      return;
    }
    if (!provider) {
      setError("请选择服务商");
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
      setError("请填写 Provider id（如 moonshot）");
      return;
    }
    if (!/^[a-z0-9][a-z0-9_-]*$/.test(id)) {
      setError("id 只能用小写字母、数字、连字符");
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
    <div className="project-dialog-backdrop" onClick={busy ? undefined : onClose}>
      <div
        className="project-dialog provider-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="project-dialog__header">OpenCode 服务商 Key</div>
        <div className="project-dialog__body">
          <p className="provider-dialog__lead">
            只写入 OpenCode 的 <code>auth.json</code>。Claude / Codex / Grok 各管各的登录，不受这里影响。
          </p>

          {/* ── List of configured providers ── */}
          {loaded && configuredList.length > 0 && (
            <div className="provider-dialog__section">
              <span className="provider-dialog__section-title">已配置的 Key</span>
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
                            title="通过 opencode auth login 登录，删除后需要重新登录"
                          >
                            OAuth
                          </span>
                        )}
                        {noProbe && (
                          <span
                            className="provider-dialog__item-badge provider-dialog__item-badge--muted"
                            title="Marionette 不会查这家余额"
                          >
                            余额不支持
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

          {loaded && configuredList.length === 0 && (
            <div className="provider-dialog__empty">还没有配置任何 API Key</div>
          )}

          {/* ── Add / Edit form ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">
              {isConfigured ? "更新 Key" : "添加 Key"}
            </span>
            {!showCustom ? (
              <>
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
                    disabled={saving || catalog.length === 0}
                  >
                    {catalog.map((p) => (
                      <option key={p.provider} value={p.provider}>
                        {p.label}
                        {p.configured || p.hasKey ? " (已配置)" : ""}
                        {p.probeStrategy === "none" || !p.probeStrategy ? " · 余额不支持" : ""}
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
                  + 添加自定义 Provider
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
                  <span className="provider-dialog__label">显示名称</span>
                  <input
                    className="provider-dialog__input"
                    value={customLabel}
                    onChange={(e) => setCustomLabel(e.target.value)}
                    placeholder="Moonshot"
                    disabled={saving}
                  />
                </label>
                <p className="provider-dialog__hint">
                  自定义端点不支持（要改 opencode.jsonc）。余额查询：不支持。
                </p>
                <button
                  type="button"
                  className="provider-dialog__link"
                  onClick={() => setShowCustom(false)}
                >
                  ← 返回目录选择
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
                placeholder={isConfigured ? "输入新 Key 覆盖已有配置" : "粘贴你的 API Key"}
                disabled={saving}
                autoFocus
              />
            </label>
            {!showCustom && selected && probeUnsupported && (
              <div className="provider-dialog__hint">
                该服务商<strong>不支持</strong>在 Marionette 内查余额（探测只实现了 DeepSeek / OpenRouter / OpenCode Zen）。
              </div>
            )}
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
          <button type="button" className="project-dialog__cancel" onClick={onClose} disabled={busy}>
            关闭
          </button>
          {showCustom ? (
            <button
              type="button"
              className="project-dialog__submit"
              disabled={busy}
              onClick={() => void handleAddCustom()}
            >
              {saving ? "保存中…" : "添加 Provider"}
            </button>
          ) : (
            <button
              type="button"
              className="project-dialog__submit"
              disabled={busy || !provider}
              onClick={() => void handleSave()}
            >
              {saving
                ? "保存中…"
                : saveConfirmed
                  ? "确认覆盖 OAuth"
                  : isConfigured
                    ? "更新 Key"
                    : "保存 Key"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
