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
import { t, useT, type DictKey } from "../lib/i18n";
import { refreshModelSources, useModelLibrary } from "../lib/modelLibrary";
import type { AgentConfig } from "../lib/types";
import { DialogShell } from "./DialogShell";

type Props = {
  agents: AgentConfig[];
  onClose: () => void;
};

/** A pending destructive action awaiting a second click (ProviderConfigDialog pattern). */
type Confirming = { id: string };

const KIND_BADGES: Record<BridgeEntryKind, DictKey> = {
  "key-plan": "creds.kindApi",
  subscription: "creds.kindLogin",
  reference: "creds.kindExternalApi",
};

function scanStatusText(source: BridgeScanSource, foundCount: number): string {
  if (source.status === "found") {
    return foundCount > 0
      ? t("creds.scanFoundSome", { count: foundCount })
      : t("creds.scanFound");
  }
  if (source.status === "restricted") return t("creds.scanRestricted");
  return t("creds.scanMissing");
}

/** One-line engine status text (未安装 / 下载中 x% / 运行中 端口+版本 / 失败原因). */
function engineStatusText(engine: EngineStatus | null): string {
  const mb = (n: number) => `${(n / 1024 / 1024).toFixed(1)} MB`;
  switch (engine?.state) {
    case "downloading":
      return engine.total > 0
        ? t("creds.engineDownloadingPct", {
            pct: Math.round((engine.downloaded / engine.total) * 100),
            got: mb(engine.downloaded),
            total: mb(engine.total),
          })
        : t("creds.engineDownloading", { got: mb(engine.downloaded) });
    case "starting":
      return t("creds.engineStarting");
    case "running":
      return engine.reused
        ? t("creds.engineRunningReused", {
            port: engine.port,
            version: engine.version ? ` · ${engine.version}` : "",
          })
        : t("creds.engineRunning", {
            port: engine.port,
            version: engine.version ? ` · ${engine.version}` : "",
          });
    case "failed":
      return t("creds.engineFailed", { error: engine.error });
    default:
      return t("creds.engineMissing");
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
  const t = useT();

  const checkSource = async (id: string) => {
    setChecks(previous => ({ ...previous, [id]: { state: t("creds.checking") } }));
    try {
      if (!isTauriRuntime()) throw new Error(t("creds.checkDesktopOnly"));
      const result = await invoke<{ state: string; detail: string }>("model_library_check_source", { id });
      setChecks(previous => ({ ...previous, [id]: result }));
    } catch (e) {
      setChecks(previous => ({
        ...previous,
        [id]: { state: t("creds.checkFailed"), detail: e instanceof Error ? e.message : String(e) },
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
      if (!report) setError(t("creds.scanIncomplete"));
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
      setError(t("creds.needApiKey"));
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
    <DialogShell
      className="project-dialog-backdrop"
      dialogClassName="project-dialog provider-dialog credentials-dialog"
      label={t("creds.dialogLabel")}
      onClose={() => {
        if (!busy) onClose();
      }}
    >
        <div className="project-dialog__header">{t("creds.dialogLabel")}</div>
        <div className="project-dialog__body">
          <p className="provider-dialog__lead">
            {t("creds.lead")}
          </p>

          {/* ── Credential list ── */}
          <div className="provider-dialog__section">
            <div className="credentials-dialog__toolbar">
              <button type="button" className="engine-popover__link" disabled={scanning || busy || Object.values(checks).some(check => check.state === t("creds.checking"))} onClick={() => void load()}>{scanning ? t("creds.refreshing") : t("creds.refresh")}</button>
              <button type="button" className="project-dialog__submit" disabled={busy} onClick={() => setAdding(true)}>{t("creds.addApi")}</button>
            </div>
            <div className="provider-dialog__list">
              {loaded && entries.length === 0 && discoveredSources.length === 0 && (
                <div className="provider-dialog__empty">{t("creds.empty")}</div>
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
                      <span className="credentials-dialog__entry-label">{entry.label.replace(t("creds.grokChannelPrefix"), "Grok · ")}</span>
                      <span className="provider-dialog__item-badge provider-dialog__item-badge--muted">
                        {KIND_BADGES[entry.kind] ? t(KIND_BADGES[entry.kind]) : entry.kind}
                      </span>
                    </span>
                    <span className="credentials-dialog__entry-status" title={check?.detail || source?.issue || entry.note || undefined}>
                      {check?.state || (source?.issue ? t("creds.statusIssuePending") : source ? t("creds.statusUnchecked") : entry.kind === "subscription" ? t("creds.statusSavedLogin") : t("creds.statusConfigured"))}
                    </span>
                    {source && <button type="button" className="engine-popover__link" disabled={scanning || check?.state === t("creds.checking")} onClick={() => void checkSource(source.id)}>{t("creds.check")}</button>}
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
                            ? t("creds.confirmDeleteTitle")
                            : t("creds.deleteKeyTitle", { name: entry.label })
                        }
                      >
                        {deleting === entry.id ? t("creds.deleting") : pendingDelete ? t("creds.confirmDelete") : t("common.delete")}
                      </button>
                    )}
                  </div>
                );
              })}
              {discoveredSources.filter(source => !entries.some(entry => sourceForEntry(entry)?.id === source.id)).map(source => <div key={source.id} className="provider-dialog__item">
                <span className="provider-dialog__item-name" title={source.endpoint}><span className="credentials-dialog__entry-label">{source.label}</span><span className="provider-dialog__item-badge provider-dialog__item-badge--muted">{t("creds.kindExternalApi")}</span></span>
                <span className="credentials-dialog__entry-status" title={checks[source.id]?.detail || source.issue || undefined}>{checks[source.id]?.state || (source.issue ? t("creds.statusIssuePending") : t("creds.statusUnchecked"))}</span>
                <button type="button" className="engine-popover__link" disabled={scanning || checks[source.id]?.state === t("creds.checking")} onClick={() => void checkSource(source.id)}>{t("creds.check")}</button>
              </div>)}
            </div>
            {entries.some((e) => e.kind !== "key-plan") && (
              <p className="provider-dialog__hint">
                {t("creds.externalCredsHint")}
              </p>
            )}
          </div>

          <details className="credentials-dialog__paths">
            <summary>{t("creds.diagnostics")}</summary>
          {/* ── Scan results (read-only) ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">{t("creds.localSources")} <button type="button" className="engine-popover__link" disabled={scanning || busy} onClick={() => void load()}>{scanning ? t("creds.scanning") : t("creds.rescan")}</button></span>
            <div className="credentials-dialog__scan">
              {loaded && scanSources.length === 0 && (
                <div className="provider-dialog__empty">{t("creds.scanNoResults")}</div>
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
                <span className="credentials-dialog__source-count">{source.models.length ? t("creds.modelCount", { count: source.models.length }) : t("creds.noModelsListed")}</span>
              </div>
              <code>{source.endpoint}</code>
              {source.origins.length > 0 && <div>{source.origins.join(t("creds.originsSeparator"))}</div>}
              {source.issue && <div className="credentials-dialog__source-issue">{source.issue}</div>}
            </div>)}
            <div className="credentials-dialog__paths">
              <details>
                <summary>{t("creds.scanPaths", { count: paths.length })}</summary>
                {paths.map(path => <div key={path.path} className="credentials-dialog__source-detail"><code>{path.path}</code><div>{path.detail}</div></div>)}
              </details>
            </div>
            {scanSources.filter(s => s.detail && s.status !== "found").map(s => <p key={s.agent} className="provider-dialog__hint">{t("creds.agentDetail", { agent: agentLabel(s.agent), detail: s.detail })}</p>)}
            {warnings.map(w => <p key={w} className="provider-dialog__hint">{w}</p>)}
          </div>

          {/* ── M2 translation engine (subscription cross-use) ── */}
          <div className="provider-dialog__section">
            <span className="provider-dialog__section-title">{t("creds.engineSection")}</span>
            <p className="provider-dialog__hint">
              {t("creds.engineHint")}
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
                <span className="credentials-dialog__scan-name">{t("creds.magpieEngine")}</span>
                <span className="credentials-dialog__scan-status">
                  {engineStatusText(engine)}
                </span>
                {engine?.state !== "running" && (
                  <button
                    type="button"
                    className="project-dialog__submit"
                    disabled={engineBusy || saving}
                    onClick={() => void handleEngineEnable()}
                    title={t("creds.engineEnableTitle")}
                  >
                    {engineBusy ? t("creds.preparing") : engine?.state === "failed" ? t("common.retry") : t("creds.enable")}
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
            <span className="provider-dialog__section-title">{t("creds.addKeySection")}</span>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">{t("creds.provider")}</span>
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
                <option value="custom">{t("creds.custom")}</option>
              </select>
            </label>
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">{preset === "custom" ? t("creds.sourceName") : t("creds.nameOptional")}</span>
              <input
                className="provider-dialog__input"
                type="text"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder={t("creds.labelPlaceholder")}
                disabled={saving}
              />
            </label>
            {preset === "custom" && <>
              <label className="provider-dialog__field"><span className="provider-dialog__label">{t("creds.endpoint")}</span><input className="provider-dialog__input" type="url" value={endpoint} onChange={e => setEndpoint(e.target.value)} placeholder="https://ollama.com/v1" disabled={saving} /></label>
              <label className="provider-dialog__field"><span className="provider-dialog__label">{t("creds.protocol")}</span><select className="provider-dialog__select" value={protocol} onChange={e => setProtocol(e.target.value)} disabled={saving}><option value="chat">OpenAI Chat Completions</option><option value="responses">OpenAI Responses</option><option value="anthropic">Anthropic Messages</option></select></label>
              <label className="provider-dialog__field"><span className="provider-dialog__label">{t("creds.modelIdsLabel")}</span><textarea className="provider-dialog__input" value={modelIds} onChange={e => setModelIds(e.target.value)} placeholder="deepseek-v4.1-flash:1017" rows={3} disabled={saving} /></label>
              <p className="provider-dialog__hint">{t("creds.endpointHint")}</p>
            </>}
            <label className="provider-dialog__field">
              <span className="provider-dialog__label">API Key{preset === "custom" ? t("creds.apiKeyOptional") : ""}</span>
              <input
                className="provider-dialog__input"
                type="password"
                value={apiKey}
                onChange={(e) => setApiKey(e.target.value)}
                placeholder={t("creds.apiKeyPlaceholder")}
                disabled={saving}
                autoFocus
              />
            </label>
          </div>}
        </div>
        <div className="project-dialog__actions">
          {adding && <button type="button" className="project-dialog__cancel" disabled={busy} onClick={() => { setAdding(false); setApiKey(""); setError(""); }}>{t("creds.cancelAdd")}</button>}
          <button type="button" className="project-dialog__cancel" onClick={onClose} disabled={busy}>
            {t("common.close")}
          </button>
          {adding && <button
            type="button"
            className="project-dialog__submit"
            disabled={busy}
            onClick={() => void handleAdd()}
          >
            {saving ? t("creds.saving") : t("creds.saveApi")}
          </button>}
        </div>
    </DialogShell>
  );
}
