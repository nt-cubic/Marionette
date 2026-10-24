import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import {
  bridgeAddKey,
  bridgeAddCustomProvider,
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
import { refreshModelSources, useModelLibrary } from "../lib/modelLibrary";
import type { AgentConfig } from "../lib/types";

type Props = {
  agents: AgentConfig[];
  onClose: () => void;
};

/** A pending destructive action awaiting a second click (ProviderConfigDialog pattern). */
type Confirming = { id: string };

const KIND_BADGES: Record<BridgeEntryKind, string> = {
  "key-plan": "API",
  subscription: "登录",
  reference: "外部 API",
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
  const { sources: discoveredSources, paths, warnings, aliases, error: discoveryError } = useModelLibrary(false);
  const [adding, setAdding] = useState(false);
  const [scanSources, setScanSources] = useState<BridgeScanSource[]>([]);
  const [entries, setEntries] = useState<BridgeEntryView[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [preset, setPreset] = useState<string>(BRIDGE_PRESETS[0].id);
  const [label, setLabel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [protocol, setProtocol] = useState("chat");
  const [modelIds, setModelIds] = useState("");
  const [scanning, setScanning] = useState(false);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Confirming | null>(null);
  const [error, setError] = useState("");
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [checks, setChecks] = useState<Record<string, { state: string; detail?: string }>>({});

  const checkSource = async (id: string) => {
    setChecks(previous => ({ ...previous, [id]: { state: "检测中…" } }));
    try {
      if (!isTauriRuntime()) throw new Error("请在桌面应用中检测");
      const result = await invoke<{ state: string; detail: string }>("model_library_check_source", { id });
      setChecks(previous => ({ ...previous, [id]: result }));
    } catch (e) {
      setChecks(previous => ({
        ...previous,
        [id]: { state: "检测失败", detail: e instanceof Error ? e.message : String(e) },
      }));
    }
  };

  const load = useCallback(async () => {
    setScanning(true);
    setChecks({});
    setError("");
    try {
      const [report, list] = await Promise.all([bridgeScan(), bridgeListEntries(), refreshModelSources()]);
      setScanSources(report?.sources ?? []);
      setEntries([...list, ...(report?.entries ?? []).filter(e => !list.some(own => own.id === e.id))]);
      if (!report) setError("凭据扫描未能完成，请重新扫描查看结果");
    } catch (e) { setError(String(e)); }
    finally { setLoaded(true); setScanning(false); }
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
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
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
    if (!key && preset !== "custom") {
      setError("请输入 API Key");
      return;
    }
    setSaving(true);
    setError("");
    try {
      if (preset === "custom") {
        await bridgeAddCustomProvider(label.trim(), key, endpoint.trim(), protocol, modelIds.split(/[,\n]/).map(s => s.trim()).filter(Boolean));
      } else { await bridgeAddKey(preset, label.trim() || null, key); }
      setApiKey("");
      setLabel("");
      setAdding(false);
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
  const sourceForEntry = (entry: BridgeEntryView) => discoveredSources.find(source => {
    const canonical = aliases[`entry:${entry.id}`];
    if (canonical && canonical === (aliases[`engine:${source.id}`] || `engine:${source.id}`)) return true;
    const channel = entry.id.startsWith("grok/channel/") ? entry.id.slice("grok/channel/".length) : null;
    return channel !== null && source.origins.some(origin => origin.startsWith("grok-build · ") && origin.endsWith(`#${channel}`));
  });

  return (
    <div className="project-dialog-backdrop" onClick={busy ? undefined : onClose}>
      <div className="project-dialog provider-dialog credentials-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="project-dialog__header">API 与登录凭证</div>
        <div className="project-dialog__body">
          <p className="provider-dialog__lead">
            管理 API 密钥和已登录账号，自动读取本机 CLI 凭证。
          </p>

          {/* ── Credential list ── */}
          <div className="provider-dialog__section">
            <div className="credentials-dialog__toolbar">
              <button type="button" className="engine-popover__link" disabled={scanning || busy || Object.values(checks).some(check => check.state === "检测中…")} onClick={() => void load()}>{scanning ? "刷新中…" : "刷新凭证"}</button>
              <button type="button" className="project-dialog__submit" disabled={busy} onClick={() => setAdding(true)}>＋ 添加 API</button>
            </div>
            <div className="provider-dialog__list">
              {loaded && entries.length === 0 && discoveredSources.length === 0 && (
                <div className="provider-dialog__empty">还没有任何凭据</div>
              )}
              {entries.map((entry) => {
                const pendingDelete = confirming?.id === entry.id;
                const canDelete = entry.kind === "key-plan";
                const source = sourceForEntry(entry);
                const check = source ? checks[source.id] : undefined;
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
                      <span className="credentials-dialog__entry-label">{entry.label.replace("Grok 自定义 channel ", "Grok · ")}</span>
                      <span className="provider-dialog__item-badge provider-dialog__item-badge--muted">
                        {KIND_BADGES[entry.kind] ?? entry.kind}
                      </span>
                    </span>
                    <span className="credentials-dialog__entry-status" title={check?.detail || source?.issue || entry.note || undefined}>
                      {check?.state || (source?.issue ? "配置待处理" : source ? "未检测" : entry.kind === "subscription" ? "已保存登录态" : "已配置")}
                    </span>
                    {source && <button type="button" className="engine-popover__link" disabled={scanning || check?.state === "检测中…"} onClick={() => void checkSource(source.id)}>检测</button>}
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
              {discoveredSources.filter(source => !entries.some(entry => sourceForEntry(entry)?.id === source.id)).map(source => <div key={source.id} className="provider-dialog__item">
                <span className="provider-dialog__item-name" title={source.endpoint}><span className="credentials-dialog__entry-label">{source.label}</span><span className="provider-dialog__item-badge provider-dialog__item-badge--muted">外部 API</span></span>
                <span className="credentials-dialog__entry-status" title={checks[source.id]?.detail || source.issue || undefined}>{checks[source.id]?.state || (source.issue ? "配置待处理" : "未检测")}</span>
                <button type="button" className="engine-popover__link" disabled={scanning || checks[source.id]?.state === "检测中…"} onClick={() => void checkSource(source.id)}>检测</button>
              </div>)}
            </div>
            {entries.some((e) => e.kind !== "key-plan") && (
              <p className="provider-dialog__hint">
                外部凭证的登录、修改和退出，请在对应 CLI 中操作。
              </p>
            )}
          </div>

          <details className="credentials-dialog__paths">
            <summary>详情与诊断</summary>
          {/* ── Scan results (read-only) ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">本机来源 <button type="button" className="engine-popover__link" disabled={scanning || busy} onClick={() => void load()}>{scanning ? "扫描中…" : "重新扫描"}</button></span>
            <div className="credentials-dialog__scan">
              {loaded && scanSources.length === 0 && (
                <div className="provider-dialog__empty">没有取得凭据扫描结果，请重新扫描查看错误信息</div>
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
            {discoveredSources.map(source => <div key={source.id} className="credentials-dialog__source-detail">
              <div className="credentials-dialog__source-head">
                <strong>{source.label}</strong>
                <span className="credentials-dialog__source-count">{source.models.length ? `${source.models.length} 个模型` : "未列出模型"}</span>
              </div>
              <code>{source.endpoint}</code>
              {source.origins.length > 0 && <div>{source.origins.join("；")}</div>}
              {source.issue && <div className="credentials-dialog__source-issue">{source.issue}</div>}
            </div>)}
            <div className="credentials-dialog__paths">
              <details>
                <summary>扫描位置与结果（{paths.length} 项）</summary>
                {paths.map(path => <div key={path.path} className="credentials-dialog__source-detail"><code>{path.path}</code><div>{path.detail}</div></div>)}
              </details>
            </div>
            {scanSources.filter(s => s.detail && s.status !== "found").map(s => <p key={s.agent} className="provider-dialog__hint">{agentLabel(s.agent)}：{s.detail}</p>)}
            {warnings.map(w => <p key={w} className="provider-dialog__hint">{w}</p>)}
          </div>

          {/* ── M2 translation engine (subscription cross-use) ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">翻译引擎</span>
            <p className="provider-dialog__hint">
              订阅登录态（Grok / Claude / ChatGPT）经内嵌 magpie
              引擎翻译为标准协议；出厂种子随应用内嵌，本机落位后可在左下角「模型共享」里检查/更新到上游稳定版。只在本机环回监听、随本应用退出。
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

          </details>
          {discoveryError && <p role="alert" className="provider-dialog__error">{discoveryError}</p>}
          {error && <div role="alert" className="provider-dialog__error">{error}</div>}
          {/* ── Add key form ── */}
          {adding && <div className="provider-dialog__section">
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
                <option value="custom">自定义</option>
              </select>
            </label>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">{preset === "custom" ? "来源名称" : "名称（可选）"}</span>
              <input
                className="provider-dialog__input"
                type="text"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="如：工作号"
                disabled={saving}
              />
            </label>
            {preset === "custom" && <>
              <label className="provider-dialog__field"><span className="provider-dialog__label">接口地址</span><input className="provider-dialog__input" type="url" value={endpoint} onChange={e => setEndpoint(e.target.value)} placeholder="https://ollama.com/v1" disabled={saving} /></label>
              <label className="provider-dialog__field"><span className="provider-dialog__label">协议</span><select className="provider-dialog__select" value={protocol} onChange={e => setProtocol(e.target.value)} disabled={saving}><option value="chat">OpenAI Chat Completions</option><option value="responses">OpenAI Responses</option><option value="anthropic">Anthropic Messages</option></select></label>
              <label className="provider-dialog__field"><span className="provider-dialog__label">模型 ID（每行一个，留空则尝试读取服务端目录）</span><textarea className="provider-dialog__input" value={modelIds} onChange={e => setModelIds(e.target.value)} placeholder="deepseek-v4.1-flash:1017" rows={3} disabled={saving} /></label>
              <p className="provider-dialog__hint">填写接口基础地址。OpenAI 兼容服务通常以 /v1 结尾；Anthropic 服务按供应商说明填写。相同名称、地址和协议再次保存会更新密钥。</p>
            </>}
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">API Key{preset === "custom" ? "（无需认证的服务可留空）" : ""}</span>
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
          </div>}
        </div>
        <div className="project-dialog__actions">
          {adding && <button type="button" className="project-dialog__cancel" disabled={busy} onClick={() => { setAdding(false); setApiKey(""); setError(""); }}>取消添加</button>}
          <button type="button" className="project-dialog__cancel" onClick={onClose} disabled={busy}>
            关闭
          </button>
          {adding && <button
            type="button"
            className="project-dialog__submit"
            disabled={busy}
            onClick={() => void handleAdd()}
          >
            {saving ? "保存中…" : "保存 API"}
          </button>}
        </div>
      </div>
    </div>
  );
}
