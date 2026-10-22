import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  bridgeAddKey,
  bridgeDeleteEntry,
  bridgeListEntries,
  bridgeScan,
  engineEnable,
  engineStatus,
  isTauriRuntime,
  type BridgeEntryKind,
  type BridgeEntryView,
  type BridgeScanSource,
  type EngineStatus,
} from "../lib/api";
import { BRIDGE_PRESETS, invalidateBridgeModels } from "../lib/bridge";
import type { AgentConfig } from "../lib/types";

type Props = {
  agents: AgentConfig[];
  onClose: () => void;
};

/** A pending destructive action awaiting a second click (ProviderConfigDialog pattern). */
type Confirming = { id: string };

const KIND_BADGES: Record<BridgeEntryKind, string> = {
  "key-plan": "密钥",
  subscription: "订阅",
  reference: "引用",
};

function scanStatusText(source: BridgeScanSource, foundCount: number): string {
  if (source.status === "found") {
    return foundCount > 0 ? `已发现 ${foundCount} 个来源` : "已发现";
  }
  if (source.status === "restricted") return "存储方式受限";
  return "未检测到";
}

/** One-line engine status text (未安装 / 下载中 x% / 运行中 端口+版本 / 失败原因). */
function engineStatusText(engine: EngineStatus | null): string {
  const mb = (n: number) => `${(n / 1024 / 1024).toFixed(1)} MB`;
  switch (engine?.state) {
    case "downloading":
      return engine.total > 0
        ? `下载中 ${Math.round((engine.downloaded / engine.total) * 100)}%（${mb(
            engine.downloaded,
          )} / ${mb(engine.total)}）`
        : `下载中…（已收 ${mb(engine.downloaded)}）`;
    case "starting":
      return "启动中…（正在拉起本地网关）";
    case "running":
      return engine.reused
        ? `运行中 · 端口 ${engine.port}（复用本机已装的 magpie${
            engine.version ? ` · ${engine.version}` : ""
          }）`
        : `运行中 · 端口 ${engine.port}${engine.version ? ` · ${engine.version}` : ""}`;
    case "failed":
      return `失败：${engine.error}`;
    default:
      return "未安装（约 48 MB，启用时从 GitHub 下载）";
  }
}

export function CredentialsDialog({ agents, onClose }: Props) {
  const [scanSources, setScanSources] = useState<BridgeScanSource[]>([]);
  const [entries, setEntries] = useState<BridgeEntryView[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [preset, setPreset] = useState<string>(BRIDGE_PRESETS[0].id);
  const [label, setLabel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Confirming | null>(null);
  const [error, setError] = useState("");
  const [engine, setEngine] = useState<EngineStatus | null>(null);

  const load = useCallback(async () => {
    const [report, list] = await Promise.all([bridgeScan(), bridgeListEntries()]);
    setScanSources(report?.sources ?? []);
    setEntries(list);
    setLoaded(true);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // ── M2 translation engine: initial snapshot + event-driven updates ──
  // Progress arrives as `engine-download-progress`; every other transition as
  // `engine-state-changed`. Reaching running also refreshes the picker cache
  // (the engine source becomes visible to the model panel).
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    const unlisteners: Array<() => void> = [];
    void engineStatus().then((snapshot) => {
      if (!disposed && snapshot) setEngine(snapshot);
    });
    void listen<{ downloaded: number; total: number }>(
      "engine-download-progress",
      (event) => {
        setEngine({ state: "downloading", ...event.payload });
      },
    ).then((dispose) => unlisteners.push(dispose));
    void listen<EngineStatus>("engine-state-changed", (event) => {
      setEngine(event.payload);
      if (event.payload.state === "running") invalidateBridgeModels();
    }).then((dispose) => unlisteners.push(dispose));
    return () => {
      disposed = true;
      unlisteners.forEach((dispose) => dispose());
    };
  }, []);

  const engineBusy =
    engine?.state === "downloading" || engine?.state === "starting";

  const handleEngineEnable = async () => {
    setSaving(true);
    setError("");
    try {
      const snapshot = await engineEnable();
      if (snapshot) setEngine(snapshot);
    } finally {
      setSaving(false);
    }
  };

  const agentLabel = useCallback(
    (agentId: string) => agents.find((a) => a.id === agentId)?.label ?? agentId,
    [agents],
  );

  /** Rows the credentials store holds for one scanned agent (`scan:<origin>`). */
  const foundCountFor = useCallback(
    (source: BridgeScanSource) =>
      entries.filter((entry) => entry.source === `scan:${source.agent}`).length,
    [entries],
  );

  const handleAdd = async () => {
    const key = apiKey.trim();
    if (!key) {
      setError("请输入 API Key");
      return;
    }
    setSaving(true);
    setError("");
    try {
      await bridgeAddKey(preset, label.trim() || null, key);
      setApiKey("");
      setLabel("");
      invalidateBridgeModels();
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (entry: BridgeEntryView) => {
    if (confirming?.id !== entry.id) {
      setConfirming({ id: entry.id });
      setError("");
      return;
    }
    setDeleting(entry.id);
    setError("");
    try {
      await bridgeDeleteEntry(entry.id);
      setConfirming(null);
      invalidateBridgeModels();
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDeleting(null);
    }
  };

  const busy = saving || deleting !== null;

  return (
    <div className="project-dialog-backdrop" onClick={busy ? undefined : onClose}>
      <div className="project-dialog provider-dialog credentials-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="project-dialog__header">凭据 · 共享密钥与来源</div>
        <div className="project-dialog__body">
          <p className="provider-dialog__lead">
            密钥加密保存在本地（<code>~/.marionette</code>），可跨 agent 直连使用；扫描只读各家 CLI 已保存的登录态，不会复制或改写。
          </p>

          {/* ── Scan results (read-only) ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">本机来源</span>
            <div className="credentials-dialog__scan">
              {loaded && scanSources.length === 0 && (
                <div className="provider-dialog__empty">未扫描到任何 agent 凭据</div>
              )}
              {scanSources.map((source) => (
                <div
                  key={source.agent}
                  className="credentials-dialog__scan-row"
                  title={source.detail || undefined}
                >
                  <span className={`credentials-dialog__scan-dot is-${source.status}`} aria-hidden />
                  <span className="credentials-dialog__scan-name">{agentLabel(source.agent)}</span>
                  <span className="credentials-dialog__scan-status">
                    {scanStatusText(source, foundCountFor(source))}
                  </span>
                </div>
              ))}
            </div>
          </div>

          {/* ── M2 translation engine (subscription cross-use) ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">翻译引擎</span>
            <p className="provider-dialog__hint">
              订阅登录态（Grok / Claude / ChatGPT）经 magpie
              引擎翻译为标准协议；出厂种子随应用内嵌，本机落位后可在左下角「模型共享」里检查/更新到上游稳定版。只在环回监听，随本应用退出。
            </p>
            <div className="credentials-dialog__scan">
              <div
                className="credentials-dialog__scan-row"
                title={engine?.state === "failed" ? engine.error : undefined}
              >
                <span
                  className={`credentials-dialog__scan-dot ${
                    engine?.state === "running"
                      ? "is-found"
                      : engine?.state === "failed"
                        ? "is-restricted"
                        : "is-missing"
                  }`}
                  aria-hidden
                />
                <span className="credentials-dialog__scan-name">magpie 引擎</span>
                <span className="credentials-dialog__scan-status">
                  {engineStatusText(engine)}
                </span>
                {engine?.state !== "running" && (
                  <button
                    type="button"
                    className="project-dialog__submit"
                    disabled={engineBusy || saving}
                    onClick={() => void handleEngineEnable()}
                    title="下载并启动翻译引擎（幂等，重复点击安全）"
                  >
                    {engineBusy ? "准备中…" : engine?.state === "failed" ? "重试" : "启用"}
                  </button>
                )}
              </div>
            </div>
          </div>

          {/* ── Credential list ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">我的密钥</span>
            <div className="provider-dialog__list">
              {loaded && entries.length === 0 && (
                <div className="provider-dialog__empty">还没有任何凭据</div>
              )}
              {entries.map((entry) => {
                const pendingDelete = confirming?.id === entry.id;
                const canDelete = entry.kind === "key-plan";
                return (
                  <div key={entry.id} className="provider-dialog__item">
                    <span
                      className="provider-dialog__item-name"
                      title={entry.note || undefined}
                    >
                      <span
                        className={
                          canDelete
                            ? "provider-dialog__item-dot"
                            : "provider-dialog__item-dot credentials-dialog__dot--borrowed"
                        }
                        aria-hidden
                      />
                      {entry.label}
                      {entry.preset && (
                        <span className="provider-dialog__item-badge">{entry.preset}</span>
                      )}
                      <span className="provider-dialog__item-badge provider-dialog__item-badge--muted">
                        {KIND_BADGES[entry.kind] ?? entry.kind}
                      </span>
                      {entry.maskedKey && (
                        <span className="credentials-dialog__masked">{entry.maskedKey}</span>
                      )}
                      {entry.source.startsWith("scan:") ? (
                        <span className="provider-dialog__item-id">扫描</span>
                      ) : (
                        <span className="provider-dialog__item-id">手动</span>
                      )}
                    </span>
                    {canDelete && (
                      <button
                        className={
                          pendingDelete
                            ? "provider-dialog__delete-btn is-confirming"
                            : "provider-dialog__delete-btn"
                        }
                        type="button"
                        onClick={() => void handleDelete(entry)}
                        disabled={deleting === entry.id}
                        title={
                          pendingDelete
                            ? "再次点击确认删除"
                            : `删除 ${entry.label} 的密钥`
                        }
                      >
                        {deleting === entry.id ? "删除中…" : pendingDelete ? "确认删除" : "删除"}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
            {entries.some((e) => e.kind !== "key-plan") && (
              <p className="provider-dialog__hint">
                订阅 / 引用型凭据保存在各家 agent 自己的文件里，只能查看状态，不支持在这里删除。
              </p>
            )}
          </div>

          {/* ── Add key form ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">添加密钥</span>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">服务商</span>
              <select
                className="provider-dialog__select"
                value={preset}
                onChange={(e) => {
                  setPreset(e.target.value);
                  setConfirming(null);
                  setError("");
                }}
                disabled={saving}
              >
                {BRIDGE_PRESETS.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.label}
                  </option>
                ))}
              </select>
            </label>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">名称（可选）</span>
              <input
                className="provider-dialog__input"
                type="text"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="如：工作号"
                disabled={saving}
              />
            </label>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">API Key</span>
              <input
                className="provider-dialog__input"
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder="粘贴 API Key（保存后不再显示）"
                disabled={saving}
                autoFocus
              />
            </label>
            {error && <div className="provider-dialog__error">{error}</div>}
          </div>
        </div>
        <div className="project-dialog__actions">
          <button type="button" className="project-dialog__cancel" onClick={onClose} disabled={busy}>
            关闭
          </button>
          <button
            type="button"
            className="project-dialog__submit"
            disabled={busy}
            onClick={() => void handleAdd()}
          >
            {saving ? "保存中…" : "添加密钥"}
          </button>
        </div>
      </div>
    </div>
  );
}
