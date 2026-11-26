import { Fragment, useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { ArrowDown, ArrowUp, ChevronRight, Plus, RefreshCw, Search, X } from "lucide-react";
import {
  bridgeAddKey,
  bridgeDeleteEntry,
  bridgeListEntries,
  bridgeListModels,
  bridgeScan,
  engineFamilyModels,
  engineQuota,
  engineStatus,
  isTauriRuntime,
  startAgentLogin,
  type BridgeEntryView,
  type BridgeModelsResult,
  type BridgeScanSource,
  type EngineFamilyModel,
  type EngineQuotaEntry,
  type EngineQuotaWindow,
  type EngineStatus,
} from "../lib/api";
import { BRIDGE_PRESETS, invalidateBridgeModels } from "../lib/bridge";
import { agentInkStyle } from "../lib/agentInk";
import { cachedCapabilitiesFor } from "../lib/capabilityCache";
import {
  configureSourceEnvironment,
  DEFAULT_MODEL_PREFERENCES,
  expandHiddenProviders,
  libraryGroups,
  ordered,
  refreshModelSources,
  rowHidden,
  rowKeys,
  saveModelPreferences,
  setSubscriptionShare,
  subscriptionShareEnabled,
  SUBSCRIPTION_FAMILIES,
  toggleRow,
  useModelLibrary,
  type LibraryGroup,
  type LibraryRow,
  type ModelLibraryPreferences,
  type ModelSource,
  type SubscriptionFamily,
} from "../lib/modelLibrary";
import { splitModelMeta } from "../lib/modelMenu";
import { pctLevel } from "../lib/quotaRail";
import { useT } from "../lib/i18n";
import type { AgentConfig } from "../lib/types";
import { ConfirmDialog } from "./ConfirmDialog";
import { CustomSourceDialog, type CustomSourceDraft } from "./CustomSourceDialog";

type RowFilter = "all" | "on" | "off";
type Translate = ReturnType<typeof useT>;
/** 左列选中项：订阅族 / 一个来源组 / 「添加来源」表单。 */
type Selection = { kind: "family"; family: SubscriptionFamily } | { kind: "group"; key: string } | { kind: "add" };
/** 一个模型行 + 它所属的来源组（隐藏与排序仍按组写盘）。 */
type RowRef = { row: LibraryRow; group: LibraryGroup };
/** 左列的一行：同一厂商可能来自多个 agent 的目录 / 引擎来源，展示层合成一行。 */
type DisplayGroup = { key: string; label: string; members: LibraryGroup[]; rows: RowRef[] };

/** 订阅族 → 本机负责登录的 agent（登录按钮走 `startAgentLogin`）。 */
const FAMILY_AGENT_ID = {
  claude: "claude-code",
  codex: "codex",
  grok: "grok-build",
  gemini: "antigravity",
} as const;

/**
 * 「模型设置」：模型库（显示与排序）、订阅共享（四族登录 + 共享 + 里面有什么模型）、
 * API 与自定义来源的密钥管理，全在一个对话框里（2026-11-27 统一，替代原来的
 * 「模型与来源」与「API 与登录凭证」两个入口）。左列选、右侧配；共享开关即时生效，
 * 显示/排序仍走底部保存。
 */
export function ModelSettingsDialog({ agents, onClose }: { agents: AgentConfig[]; onClose: () => void }) {
  const { preferences, sources, warnings, paths, aliases, accounts, signedIn, error: loadError } = useModelLibrary(false);
  const [draft, setDraft] = useState(preferences);
  const [catalog, setCatalog] = useState<BridgeModelsResult | null>(null);
  const [quotas, setQuotas] = useState<EngineQuotaEntry[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [selection, setSelection] = useState<Selection | null>(null);
  const [filter, setFilter] = useState<RowFilter>("all");
  // 订阅共享：开关即时落盘；pendingShare 记着待确认的族。
  const [pendingShare, setPendingShare] = useState<string | null>(null);
  const [shareBusy, setShareBusy] = useState(false);
  // 订阅族的模型清单（只读 peek：无视共享开关，问引擎那一族里有什么）。
  const [familyModels, setFamilyModels] = useState<Record<string, EngineFamilyModel[] | "loading" | "error">>({});
  const [familyNote, setFamilyNote] = useState<string | null>(null);
  // 凭据：预设 API Key 条目、本机扫描、来源检测。
  const [entries, setEntries] = useState<BridgeEntryView[]>([]);
  const [scanSources, setScanSources] = useState<BridgeScanSource[]>([]);
  const [checks, setChecks] = useState<Record<string, { state: string; detail?: string }>>({});
  const [deleting, setDeleting] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<string | null>(null);
  const [engine, setEngine] = useState<EngineStatus | null>(null);
  const [sourceDialog, setSourceDialog] = useState<{ draft: CustomSourceDraft | null } | null>(null);
  // 订阅详情的「登录」：在自己的窗口里完成，回来自动刷新一次。
  const [loginBusy, setLoginBusy] = useState<string | null>(null);
  const [loginNote, setLoginNote] = useState<Record<string, string>>({});
  const t = useT();

  useEffect(() => { if (!dirty) setDraft(preferences); }, [preferences, dirty]);

  const refresh = useCallback(async () => {
    setBusy(true); setError("");
    // 额度与订阅族的模型清单都是 advisory，绝不挡扫描：引擎没起就给空。
    void engineQuota(false).then(res => { if (res.entries.length > 0) setQuotas(res.entries); }).catch(() => undefined);
    void engineFamilyModels().then(res => {
      if (res.families) setFamilyModels(previous => {
        const next = { ...previous };
        for (const family of SUBSCRIPTION_FAMILIES) if (res.families[family]) next[family] = res.families[family]!;
        return next;
      });
      setFamilyNote(res.note ?? null);
    }).catch(() => undefined);
    try {
      await refreshModelSources();
      invalidateBridgeModels();
      setCatalog(await bridgeListModels("grok-build"));
      const [report, list] = await Promise.all([bridgeScan(), bridgeListEntries()]);
      setScanSources(report?.sources ?? []);
      const merged = [...list, ...(report?.entries ?? []).filter(e => !list.some(own => own.id === e.id))];
      setEntries(merged);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);

  // 引擎状态（诊断区）：快照 + 事件。
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    const disposers: Array<() => void> = [];
    void engineStatus().then(snapshot => { if (!disposed && snapshot) setEngine(snapshot); });
    void listen<EngineStatus>("engine-state-changed", event => setEngine(event.payload)).then(fn => { if (disposed) fn(); else disposers.push(fn); });
    return () => { disposed = true; disposers.forEach(fn => fn()); };
  }, []);

  // ── 模型库分组（显示与排序）────────────────────────────────────────────
  const groups = useMemo(() => {
    const result = new Map<string, LibraryGroup>();
    const add = (group: LibraryGroup) => {
      const previous = result.get(group.key);
      if (!previous) result.set(group.key, { ...group, rows: group.rows.slice() });
      else for (const row of group.rows) if (!previous.rows.some(r => r.key === row.key)) previous.rows.push(row);
    };
    for (const agent of agents) {
      for (const group of libraryGroups(agent.id, cachedCapabilitiesFor(agent.id)?.models ?? [], catalog, sources, aliases, accounts)) add(group);
    }
    for (const source of sources) {
      const key = aliases[`engine:${source.id}`] ?? `engine:${source.id}`;
      add({ key, label: source.label, rows: source.models.map(m => ({ key: `${key}/${m.id}`, label: m.label || m.id, modelId: m.id, origin: source.origins.join("\n") })) });
    }
    // 只有密钥、还没有模型行的条目也要能管理（删 key / 看端点）。
    for (const entry of entries) {
      if (entry.kind !== "key-plan") continue;
      if (sources.some(source => sourceForEntry(source, entry, aliases))) continue;
      add({ key: `entry:${entry.id}`, label: entry.label, rows: [] });
    }
    return ordered([...result.values()], draft.providerOrder, g => g.key, g => g.label)
      .map(g => ({ ...g, rows: ordered(g.rows, draft.modelOrder[g.key] ?? [], r => r.key, r => r.label) }));
  }, [agents, catalog, sources, aliases, accounts, entries, draft.providerOrder, draft.modelOrder]);
  useEffect(() => {
    if (draft.hiddenProviders.some(key => groups.some(g => g.key === key))) {
      setDraft(expandHiddenProviders(draft, groups));
    }
  }, [draft, groups]);

  const q = query.trim().toLowerCase();
  const loggedFamilies = useMemo(() => new Set(signedIn), [signedIn]);
  const isRegisterFailedWarning = (w: string) => w.includes("注册失败") || w.includes("registration failed");

  /**
   * 左列按厂商合并（2026-11-27 用户定案：xAI / Z.AI 在多个 agent 的目录里各出现一次，
   * 看起来就是「重叠」）。合并只发生在展示层：每行仍属于它自己的来源组
   * （`native:<agent>:<vendor>` / `engine:<source>`），显示开关与排序照旧按组写盘。
   */
  const display = useMemo(() => {
    const byLabel = new Map<string, DisplayGroup>();
    for (const group of groups) {
      const key = plain(group.label) || group.key;
      const rows = group.rows.map(row => ({ row, group }));
      const existing = byLabel.get(key);
      if (existing) {
        existing.members.push(group);
        existing.rows.push(...rows);
      } else {
        byLabel.set(key, { key: group.key, label: group.label, members: [group], rows });
      }
    }
    const out = [...byLabel.values()];
    const rank = (key: string) => { const index = draft.providerOrder.indexOf(key); return index < 0 ? Number.MAX_SAFE_INTEGER : index; };
    out.sort((a, b) => rank(a.key) - rank(b.key) || a.label.localeCompare(b.label));
    for (const entry of out) {
      const memberIndex = (ref: RowRef) => entry.members.indexOf(ref.group);
      const orderOf = (ref: RowRef) => { const index = (draft.modelOrder[ref.group.key] ?? []).indexOf(ref.row.key); return index < 0 ? Number.MAX_SAFE_INTEGER : index; };
      entry.rows.sort((x, y) => memberIndex(x) - memberIndex(y) || orderOf(x) - orderOf(y) || x.row.label.localeCompare(y.row.label));
    }
    return out;
  }, [groups, draft.providerOrder, draft.modelOrder]);
  const filtered = display.filter(d => !q || `${d.label} ${d.rows.map(r => `${r.row.label} ${r.row.modelId}`).join(" ")}`.toLowerCase().includes(q));

  const current: Selection = useMemo(() => {
    if (selection) {
      if (selection.kind === "group" && !display.some(d => d.key === selection.key)) return { kind: "family", family: "claude" };
      return selection;
    }
    return { kind: "family", family: "claude" };
  }, [selection, display]);
  const currentDisplay = current.kind === "group" ? display.find(d => d.key === current.key) ?? null : null;
  const currentSource = currentDisplay?.members.map(member => sourceOf(member, sources, aliases)).find((source): source is ModelSource => !!source);
  const currentQuota = currentDisplay?.members.map(member => quotaFor(member, sourceOf(member, sources, aliases), quotas)).find((quota): quota is SourceQuota => !!quota) ?? null;
  const currentShared = currentDisplay ? currentDisplay.rows.filter(ref => isShown(ref.group, ref.row, draft)).length : 0;
  // 成员分段标题只在彼此能区分时出现（native 的「<agent> 的模型」；来源按名字）。
  // 两个成员名字去重后一样（deepseek / DeepSeek）就当一个列表，别再画两条重名的段头。
  const memberLabels = currentDisplay ? currentDisplay.members.map(member => memberLabel(member, sources, aliases, agents, t)) : [];
  const showMembers = memberLabels.length > 1 && new Set(memberLabels.map(plain)).size === memberLabels.length;
  function toggleDisplay(entry: DisplayGroup, on: boolean) {
    const next = expandHiddenProviders(draft, groups);
    const keys = new Set(entry.rows.flatMap(ref => rowKeys(ref.row)));
    if (on) change({ ...next, hiddenModels: next.hiddenModels.filter(key => !keys.has(key)) });
    else change({ ...next, hiddenModels: [...new Set([...next.hiddenModels, ...keys])] });
  }

  // 选中订阅族时懒取它的模型清单（refresh 已并行拉过一轮；失败可单独重试）。
  const selectedFamily = current.kind === "family" ? current.family : null;
  useEffect(() => {
    if (!selectedFamily) return;
    if (familyModels[selectedFamily]) return;
    setFamilyModels(previous => ({ ...previous, [selectedFamily]: "loading" }));
    void engineFamilyModels()
      .then(res => {
        if (res.families) setFamilyNote(res.note ?? null);
        setFamilyModels(previous => ({ ...previous, [selectedFamily]: res.families?.[selectedFamily] ?? [] }));
      })
      .catch(() => setFamilyModels(previous => ({ ...previous, [selectedFamily]: "error" })));
  }, [selectedFamily, familyModels]);

  const change = (next: ModelLibraryPreferences) => { setDraft(next); setDirty(true); };
  function move(keys: string[], key: string, delta: number): string[] {
    const next = keys.slice(); const from = next.indexOf(key); const to = from + delta;
    if (from >= 0 && to >= 0 && to < next.length) [next[from], next[to]] = [next[to]!, next[from]!];
    return next;
  }
  async function save() {
    setBusy(true); setError("");
    try {
      // subscriptionShares 走专用命令即时落盘，保存时以最新权威值为准。
      await saveModelPreferences({ ...expandHiddenProviders(draft, groups), subscriptionShares: preferences.subscriptionShares });
      onClose();
    }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function applyShare(family: string, enabled: boolean) {
    setShareBusy(true); setError("");
    try {
      await setSubscriptionShare(family, enabled);
      await refresh();
    } catch (e) { setError(String(e)); }
    finally { setShareBusy(false); }
  }
  async function deleteEntry(entry: BridgeEntryView) {
    if (confirming !== entry.id) { setConfirming(entry.id); setError(""); return; }
    setDeleting(entry.id); setError("");
    try {
      await bridgeDeleteEntry(entry.id);
      setConfirming(null);
      invalidateBridgeModels();
      await refresh();
    } catch (e) { setError(String(e)); }
    finally { setDeleting(null); }
  }
  async function loginFamily(family: SubscriptionFamily) {
    const agentId = FAMILY_AGENT_ID[family];
    const agent = agents.find(candidate => candidate.id === agentId);
    if (!agent) {
      setLoginNote(previous => ({ ...previous, [family]: t("models.familyLoginNoAgent") }));
      return;
    }
    setLoginBusy(family);
    setLoginNote(previous => ({ ...previous, [family]: "" }));
    const result = await startAgentLogin(agentId);
    setLoginNote(previous => ({
      ...previous,
      [family]: result?.started ? t("models.familyLoginStarted") : result?.message || t("models.familyLoginFailed"),
    }));
    setLoginBusy(null);
    if (result?.started) window.setTimeout(() => void refresh(), 8000);
  }
  const checkSource = async (id: string) => {
    setChecks(previous => ({ ...previous, [id]: { state: t("creds.checking") } }));
    try {
      if (!isTauriRuntime()) throw new Error(t("creds.checkDesktopOnly"));
      const result = await invoke<{ state: string; detail: string }>("model_library_check_source", { id });
      setChecks(previous => ({ ...previous, [id]: result }));
    } catch (e) {
      setChecks(previous => ({ ...previous, [id]: { state: t("creds.checkFailed"), detail: e instanceof Error ? e.message : String(e) } }));
    }
  };
  const draftOf = (entry: BridgeEntryView): CustomSourceDraft | null => {
    const source = sources.find(candidate => candidate.origins.some(origin => origin.startsWith("Marionette · ") && origin.endsWith(`#${entry.id}`)));
    if (!source) return null;
    return {
      entryId: entry.id,
      label: entry.label,
      endpoint: source.endpoint,
      protocol: source.protocol as CustomSourceDraft["protocol"],
      models: source.models.map(model => model.id),
    };
  };

  // ── 保存状态与统计 ───────────────────────────────────────────────────
  const totals = useMemo(() => {
    let shared = 0, total = 0, flips = 0;
    for (const g of groups) for (const r of g.rows) {
      total++;
      const now = isShown(g, r, draft);
      if (now) shared++;
      if (now !== isShown(g, r, preferences)) flips++;
    }
    const orderChanged = JSON.stringify(draft.providerOrder) !== JSON.stringify(preferences.providerOrder) || JSON.stringify(draft.modelOrder) !== JSON.stringify(preferences.modelOrder);
    return { shared, total, changes: flips + (orderChanged ? 1 : 0) };
  }, [groups, draft, preferences]);

  const rowsInView = currentDisplay ? currentDisplay.rows.filter(ref => {
    const on = isShown(ref.group, ref.row, draft);
    if (filter === "on" && !on) return false;
    if (filter === "off" && on) return false;
    return !q || currentDisplay.label.toLowerCase().includes(q) || `${ref.row.label} ${ref.row.modelId}`.toLowerCase().includes(q);
  }) : [];

  return <div className="project-dialog-backdrop" onClick={onClose}>
    <div className="project-dialog model-library-dialog model-settings-dialog" role="dialog" aria-modal="true" aria-label={t("models.settingsTitle")} onClick={e => e.stopPropagation()}>
      <div className="model-library-dialog__header">
        <span className="model-library-dialog__title">{t("models.settingsTitle")}</span>
        <span className="model-library-dialog__status">{busy ? t("models.scanning") : t("models.sourceCount", { count: groups.length })}</span>
        <span className="model-library-dialog__spacer" />
        <button type="button" className="model-library-dialog__text-btn" disabled={busy} onClick={() => void refresh()}>{t("models.rescan")}</button>
        <button type="button" className="model-library-dialog__close" aria-label={t("common.close")} onClick={onClose}><X size={14} /></button>
      </div>
      {(error || loadError) && <p role="alert" className="model-library-dialog__error model-library-dialog__strip">{error || loadError}</p>}
      {warnings.length > 0 && <details className="model-library-dialog__warn">
        <summary>{warnings.every(isRegisterFailedWarning) ? t("models.sourcesFailed", { count: warnings.length }) : t("models.sourceNotices", { count: warnings.length })}<span className="model-library-dialog__warn-more">{t("models.view")}</span><button type="button" className="model-library-dialog__link" disabled={busy} onClick={e => { e.preventDefault(); e.stopPropagation(); void refresh(); }}>{busy ? t("creds.refreshing") : t("models.refreshRetry")}</button></summary>
        {warnings.map(w => <div key={w}>{w}</div>)}
      </details>}
      <div className="model-library-dialog__body">
        <div className="model-library-dialog__sources">
          <label className="model-library-dialog__search"><Search size={13} aria-hidden /><input type="search" placeholder={t("models.searchPlaceholder")} aria-label={t("models.searchPlaceholder")} value={query} onChange={e => setQuery(e.target.value)} /></label>
          {/* 订阅共享四族：点行看详情（登录、额度、这一族里有什么模型），右侧开关即时生效。 */}
          <div className="model-library-dialog__shares">
            <span className="model-library-dialog__section-title" title={t("models.shareFamiliesHint")}>{t("models.shareFamilies")}</span>
            {SUBSCRIPTION_FAMILIES.map(family => {
              const on = subscriptionShareEnabled(preferences, family);
              const logged = loggedFamilies.has(family);
              const chosen = current.kind === "family" && current.family === family;
              const status = on ? t("models.shareFamilyOn") : logged ? t("models.shareFamilyOff") : t(FAMILY_LOGIN_KEY[family]);
              return <div key={family} className={`model-library-dialog__share${chosen ? " is-selected" : ""}`}>
                <button type="button" className="model-library-dialog__share-pick" aria-current={chosen ? "true" : undefined} onClick={() => { setSelection({ kind: "family", family }); setFilter("all"); }}>
                  <span className="model-library-dialog__share-text">
                    <span className="model-library-dialog__share-name" title={t(FAMILY_LABEL_KEY[family])}>{t(FAMILY_LABEL_KEY[family])}</span>
                    <span className={`model-library-dialog__share-sub${on ? " is-on" : logged ? "" : " is-faint"}`} title={status}>{status}</span>
                  </span>
                </button>
                <button type="button" role="switch" className="model-library-dialog__switch" aria-checked={on} aria-label={`${t("models.shareSource")} · ${t(FAMILY_LABEL_KEY[family])}`} disabled={busy || shareBusy} onClick={() => { if (on) void applyShare(family, false); else setPendingShare(family); }}><i /></button>
              </div>;
            })}
          </div>
          <div className="model-library-dialog__section">
            <span className="model-library-dialog__section-title">{t("models.sectionSources")}</span>
            <div className="model-library-dialog__source-list">
            {filtered.length === 0 && <p className="model-library-dialog__empty">{busy ? t("models.readingCatalog") : q ? t("models.noMatches") : t("models.noSourcesYet")}</p>}
            {filtered.map(entry => {
              const memberSources = entry.members.map(member => sourceOf(member, sources, aliases));
              const shared = entry.rows.filter(ref => isShown(ref.group, ref.row, draft)).length;
              const needKey = memberSources.some(source => !!source?.missingEnv?.length);
              const issueOnly = entry.rows.length === 0 && memberSources.some(source => !!source?.issue);
              const sub = needKey ? { text: t("models.needKey"), warn: true }
                : issueOnly ? { text: t("models.sourceIssue"), warn: true }
                : entry.rows.length === 0 ? { text: t("models.noModelsYet"), warn: false }
                : shared === 0 ? { text: t("models.unshared"), warn: false }
                : { text: `${t("models.shared")} ${shared} / ${entry.rows.length}`, warn: false };
              const quota = summarizeQuota(entry.members.map(member => quotaFor(member, sourceOf(member, sources, aliases), quotas)).find((candidate): candidate is SourceQuota => !!candidate) ?? null, t);
              const chosen = current.kind === "group" && current.key === entry.key;
              return <button type="button" key={entry.key} className={`model-library-dialog__source${chosen ? " is-selected" : ""}`} aria-current={chosen ? "true" : undefined} onClick={() => { setSelection({ kind: "group", key: entry.key }); setFilter("all"); }}>
                <span className="model-library-dialog__source-text"><span className="model-library-dialog__source-name">{entry.label}</span><span className={`model-library-dialog__source-sub${sub.warn ? " is-warn" : ""}`}>{sub.text}</span></span>
                {quota.label && <span className={`model-library-dialog__source-quota ${quota.tone}`}>{quota.label}</span>}
              </button>;
            })}
            </div>
          </div>
          <div className="model-library-dialog__actions">
            <button type="button" className="model-library-dialog__text-btn" disabled={busy} onClick={() => setSelection({ kind: "add" })}><Plus size={12} aria-hidden /> {t("models.addSource")}</button>
            <button type="button" className="model-library-dialog__text-btn" disabled={busy} onClick={() => void refresh()}><RefreshCw size={12} aria-hidden /> {t("models.scanLocal")}</button>
          </div>
          <details className="model-library-dialog__diagnostics">
            <summary>{t("models.diagnosticsTitle")}</summary>
            <div className="credentials-dialog__scan">
              {scanSources.length === 0 && <div className="provider-dialog__empty">{t("creds.scanNoResults")}</div>}
              {scanSources.map(source => <div key={source.agent} className="credentials-dialog__scan-row" title={source.detail || undefined}>
                <span className={`credentials-dialog__scan-dot is-${source.status}`} aria-hidden />
                <span className="credentials-dialog__scan-name">{agentLabel(agents, source.agent)}</span>
                <span className="credentials-dialog__scan-status">{scanStatusText(source, entries, t)}</span>
              </div>)}
              {engine && <div className="credentials-dialog__scan-row" title={engine.state === "failed" ? engine.error : undefined}>
                <span className={`credentials-dialog__scan-dot is-${engine.state === "running" ? "found" : engine.state === "failed" ? "restricted" : "missing"}`} aria-hidden />
                <span className="credentials-dialog__scan-name">{t("creds.magpieEngine")}</span>
                <span className="credentials-dialog__scan-status">{engineStatusText(engine, t)}</span>
              </div>}
            </div>
            {paths.length > 0 && <details><summary>{t("creds.scanPaths", { count: paths.length })}</summary>{paths.map(path => <div key={path.path} className="credentials-dialog__source-detail"><code>{path.path}</code><div>{path.detail}</div></div>)}</details>}
          </details>
        </div>

        {current.kind === "family"
          ? <FamilyDetail family={current.family} signedIn={loggedFamilies.has(current.family)} shared={subscriptionShareEnabled(preferences, current.family)}
              models={familyModels[current.family]} note={familyNote} quota={quotaForFamily(current.family, quotas)} shareBusy={shareBusy || busy}
              agent={agents.find(candidate => candidate.id === FAMILY_AGENT_ID[current.family]) ?? null}
              loginBusy={loginBusy === current.family} loginNote={loginNote[current.family] ?? ""}
              onLogin={() => void loginFamily(current.family)}
              onToggleShare={() => { const on = subscriptionShareEnabled(preferences, current.family); if (on) void applyShare(current.family, false); else setPendingShare(current.family); }}
              onReload={() => { setFamilyModels(previous => { const next = { ...previous }; delete next[current.family]; return next; }); void refresh(); }} />
          : current.kind === "add"
            ? <AddSourceDetail sources={sources} onSaved={async () => { await refresh(); setSelection({ kind: "family", family: "claude" }); }} onCustom={() => setSourceDialog({ draft: null })} />
            : currentDisplay
              ? <div className="model-library-dialog__detail">
                <div className="model-library-dialog__detail-head">
                  <div className="model-library-dialog__detail-title">
                    <span className="model-library-dialog__detail-name">{currentDisplay.label}</span>
                    {currentDisplay.members.length === 1 && currentSource && <span className="model-library-dialog__faint">{currentSource.protocol}</span>}
                    {currentDisplay.members.length > 1 && <span className="model-library-dialog__faint">{t("models.memberCount", { count: currentDisplay.members.length })}</span>}
                    <span className="model-library-dialog__spacer" />
                    <span className="model-library-dialog__faint">{t("models.shareSource")}</span>
                    <button type="button" role="switch" className="model-library-dialog__switch" aria-checked={currentShared > 0} aria-label={t("models.shareSource")} disabled={!currentDisplay.rows.length} onClick={() => toggleDisplay(currentDisplay, currentShared === 0)}><i /></button>
                  </div>
                  {currentDisplay.members.map(member => {
                    const source = sourceOf(member, sources, aliases);
                    if (!source) return null;
                    const entry = entryForSource(source, entries, aliases);
                    if (!entry && !source.issue && !source.missingEnv?.length) return null;
                    return <Fragment key={`cred:${member.key}`}>
                      <SourceCredential label={currentDisplay.members.length > 1 ? source.label : ""} entry={entry} source={source} check={checks[source.id]} busy={busy || deleting !== null}
                        onCheck={() => void checkSource(source.id)} onEdit={() => { const d = entry ? draftOf(entry) : null; setSourceDialog({ draft: d }); }}
                        confirming={confirming} deleting={deleting} onDelete={(item) => void deleteEntry(item)} />
                      {(source.issue || source.missingEnv?.length) ? <div className="model-library-dialog__issue">
                        {source.issue && <div className="model-library-dialog__issue-text">{source.issue}</div>}
                        {source.missingEnv?.map(name => <SourceEnvironmentInput key={name} sourceId={source.id} name={name} onSaved={refresh} />)}
                      </div> : null}
                    </Fragment>;
                  })}
                  {currentDisplay.members.length === 1 && currentSource && (originAgents(currentSource, agents, t).length > 0 || currentSource.endpoint) && <div className="model-library-dialog__lend">
                    <span className="model-library-dialog__faint">{t("models.discoveredFrom")}</span>
                    {originAgents(currentSource, agents, t).map(a => <span key={a.name} className="model-library-dialog__agent" style={agentInkStyle(a.id)}>{a.name}</span>)}
                    <span className="model-library-dialog__endpoint" title={[currentSource.endpoint, ...currentSource.origins].join("\n")}>{currentSource.endpoint}</span>
                  </div>}
                  {currentQuota && (currentQuota.windows.length > 0 || currentQuota.balance) && <div className="model-library-dialog__quota">
                    {currentQuota.windows.length > 0 && <>
                      <div className="model-library-dialog__quota-head"><span>{t("models.quotaHeading")}</span><span className="model-library-dialog__faint">{quotaResetNote(currentQuota.windows[0]!.w, t)}</span></div>
                      {currentQuota.windows.map(({ w, used }, i) => <div key={w.name} className={`model-library-dialog__win${i === 0 ? " is-main" : ""} ${quotaTone(used)}`}>
                        <span>{w.name}</span><i><span style={{ width: `${used}%` }} /></i><em>{used >= 100 ? t("models.quotaFull") : t("models.quotaLeft", { pct: Math.round(100 - used) })}</em>
                      </div>)}
                    </>}
                    {currentQuota.balance && <div className="model-library-dialog__balance"><span>{t("models.balance")}</span><b>{currentQuota.balance}</b></div>}
                  </div>}
                  <div className="model-library-dialog__filters" role="group">
                    {([["all", t("models.all"), currentDisplay.rows.length], ["on", t("models.shared"), currentShared], ["off", t("models.unshared"), currentDisplay.rows.length - currentShared]] as const).map(([id, label, n]) =>
                      <button type="button" key={id} className={`model-library-dialog__chip${filter === id ? " is-active" : ""}`} aria-pressed={filter === id} onClick={() => setFilter(id)}>{label} {n}</button>)}
                  </div>
                </div>
                <div className="model-library-dialog__models">
                  {rowsInView.length === 0 && <p className="model-library-dialog__empty">{t("models.noModelsInView")}</p>}
                  {rowsInView.map((ref, index) => {
                    const { row, group } = ref;
                    const memberIndex = group.rows.findIndex(candidate => candidate.key === row.key);
                    const on = isShown(group, row, draft);
                    const reorder = (delta: number) => change({ ...draft, modelOrder: { ...draft.modelOrder, [group.key]: move(group.rows.map(candidate => candidate.key), row.key, delta) } });
                    const { name, meta } = splitModelMeta(rowLabel(currentDisplay.label, row.label, row.modelId));
                    const heading = showMembers && rowsInView[index - 1]?.group !== group;
                    return <Fragment key={row.key}>
                      {heading && <div className="model-library-dialog__member">{memberLabels[currentDisplay.members.indexOf(group)]}</div>}
                      <div className={`model-library-dialog__model${on ? " is-on" : ""}`}>
                        <label>
                          <input type="checkbox" checked={on} onChange={() => { const next = expandHiddenProviders(draft, groups); change(toggleRow(next, row)); }} />
                          <span className="model-library-dialog__name" title={row.modelId}>{name}{meta && <em>{meta}</em>}</span>
                          <small>{row.modelId}</small>
                        </label>
                        <div className="model-library-dialog__order">
                          <button type="button" aria-label={t("models.moveModelUp", { name: row.label })} disabled={memberIndex === 0} onClick={() => reorder(-1)}><ArrowUp size={13} /></button>
                          <button type="button" aria-label={t("models.moveModelDown", { name: row.label })} disabled={memberIndex === group.rows.length - 1} onClick={() => reorder(1)}><ArrowDown size={13} /></button>
                        </div>
                      </div>
                    </Fragment>;
                  })}
                </div>
              </div>
              : <div className="model-library-dialog__detail"><p className="model-library-dialog__empty">{busy ? t("models.readingCatalog") : t("models.noMatches")}</p></div>}
      </div>
      {catalog?.note && <p className="model-library-dialog__note">{catalog.note}</p>}
      <div className="model-library-dialog__footer">
        <span className="model-library-dialog__sum">{t("models.shared")} <b>{totals.shared}</b> / {totals.total}</span>
        {totals.changes > 0 && <span className="model-library-dialog__unsaved">· {t("models.unsavedCount", { count: totals.changes })}</span>}
        <span className="model-library-dialog__spacer" />
        <button type="button" className="model-library-dialog__btn model-library-dialog__btn--ghost" disabled={busy} title={t("models.resetAll")} onClick={() => change(DEFAULT_MODEL_PREFERENCES)}>{t("models.resetShort")}</button>
        <button type="button" className="model-library-dialog__btn" onClick={onClose}>{t("common.cancel")}</button>
        <button type="button" className="model-library-dialog__btn model-library-dialog__btn--primary" disabled={busy} onClick={() => void save()}>{t("common.save")}</button>
      </div>
    </div>
    {pendingShare && <ConfirmDialog
      title={t("models.shareWarnTitle", { family: t(FAMILY_LABEL_KEY[pendingShare as SubscriptionFamily]) })}
      description={t("models.shareWarnBody")}
      confirmLabel={t("models.shareWarnConfirm")}
      cancelLabel={t("models.shareWarnCancel")}
      danger={false}
      onConfirm={() => { const family = pendingShare; setPendingShare(null); void applyShare(family, true); }}
      onCancel={() => setPendingShare(null)}
    />}
    {sourceDialog && <CustomSourceDialog initial={sourceDialog.draft} onClose={() => setSourceDialog(null)} onSaved={async () => { setSourceDialog(null); await refresh(); }} />}
  </div>;
}

/** 订阅族详情：登录、额度、共享开关、这一族里有什么模型（只读 peek）。 */
function FamilyDetail({ family, signedIn, shared, models, note, quota, shareBusy, agent, loginBusy, loginNote, onLogin, onToggleShare, onReload }: {
  family: SubscriptionFamily; signedIn: boolean; shared: boolean;
  models: EngineFamilyModel[] | "loading" | "error" | undefined;
  note: string | null; quota: SourceQuota | null; shareBusy: boolean;
  agent: AgentConfig | null; loginBusy: boolean; loginNote: string;
  onLogin: () => void;
  onToggleShare: () => void; onReload: () => void;
}) {
  const t = useT();
  const modelList = Array.isArray(models) ? models : null;
  return <div className="model-library-dialog__detail">
    <div className="model-library-dialog__detail-head">
      <div className="model-library-dialog__detail-title">
        <span className="model-library-dialog__detail-name">{t(FAMILY_LABEL_KEY[family])}</span>
        <span className={`model-library-dialog__badge${signedIn ? " is-on" : ""}`}>{t(signedIn ? "models.familySignedIn" : "models.familyNotSignedIn")}</span>
        <span className="model-library-dialog__spacer" />
        <span className="model-library-dialog__faint">{t("models.shareSource")}</span>
        <button type="button" role="switch" className="model-library-dialog__switch" aria-checked={shared} aria-label={t("models.shareSource")} disabled={shareBusy} onClick={onToggleShare}><i /></button>
      </div>
      {!signedIn && <div className="model-library-dialog__login">
        <button type="button" className="model-library-dialog__btn model-library-dialog__btn--primary" disabled={loginBusy || !agent} onClick={onLogin}>{loginBusy ? t("models.familyLoginStarting") : t("models.familyLogin")}</button>
        <span className="model-library-dialog__faint">{loginNote || (agent ? t("models.familyLoginHint") : t("models.familyLoginNoAgent"))}</span>
      </div>}
      {!signedIn && <p className="model-library-dialog__hint">{t(FAMILY_LOGIN_HOW_KEY[family])}</p>}
      {quota && (quota.windows.length > 0 || quota.balance) && <div className="model-library-dialog__quota">
        {quota.windows.length > 0 && <>
          <div className="model-library-dialog__quota-head"><span>{t("models.quotaHeading")}</span><span className="model-library-dialog__faint">{quotaResetNote(quota.windows[0]!.w, t)}</span></div>
          {quota.windows.map(({ w, used }, i) => <div key={w.name} className={`model-library-dialog__win${i === 0 ? " is-main" : ""} ${quotaTone(used)}`}>
            <span>{w.name}</span><i><span style={{ width: `${used}%` }} /></i><em>{used >= 100 ? t("models.quotaFull") : t("models.quotaLeft", { pct: Math.round(100 - used) })}</em>
          </div>)}
        </>}
        {quota.balance && <div className="model-library-dialog__balance"><span>{t("models.balance")}</span><b>{quota.balance}</b></div>}
      </div>}
    </div>
    <div className="model-library-dialog__models-head">
      <span>{t("models.familyModelsTitle")}{modelList ? <b>{modelList.length}</b> : null}</span>
      <button type="button" className="model-library-dialog__text-btn" onClick={onReload}>{t("models.familyModelsRefresh")}</button>
    </div>
    <div className="model-library-dialog__models">
      {models === "loading" && <p className="model-library-dialog__empty">{t("models.familyModelsLoading")}</p>}
      {models === "error" && <p className="model-library-dialog__empty">{t("models.familyModelsFailed")}</p>}
      {modelList && modelList.length === 0 && <p className="model-library-dialog__empty">{note || t("models.familyModelsEmpty")}</p>}
      {modelList && modelList.length > 0 && <>
        <p className="model-library-dialog__hint">{t("models.familyModelsNote")}</p>
        {modelList.map(model => <div className="model-library-dialog__model is-on" key={model.id}>
          <label>
            <span className="model-library-dialog__name" title={model.id}>{model.displayName || model.id}</span>
            <small>{model.id}</small>
          </label>
          <span className="model-library-dialog__model-meta">
            {model.contextWindow > 0 && <em>{fmtTokens(model.contextWindow)}</em>}
            {model.images && <em>{t("models.familyModelVision")}</em>}
            {model.levels.length > 0 && <em>{model.levels.join(" / ")}</em>}
          </span>
        </div>)}
      </>}
    </div>
  </div>;
}

/** 添加来源：预设 API Key（与旧凭据页同一个表单）或自定义来源对话框。 */
function AddSourceDetail({ sources, onSaved, onCustom }: { sources: ModelSource[]; onSaved: () => Promise<void>; onCustom: () => void }) {
  const t = useT();
  const [preset, setPreset] = useState<string>(BRIDGE_PRESETS[0].id);
  const [label, setLabel] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const add = async () => {
    const key = apiKey.trim();
    if (!key) { setError(t("creds.needApiKey")); return; }
    setSaving(true); setError("");
    try {
      await bridgeAddKey(preset, label.trim() || null, key);
      setApiKey(""); setLabel("");
      invalidateBridgeModels();
      await onSaved();
    } catch (e) { setError(String(e)); }
    finally { setSaving(false); }
  };
  return <div className="model-library-dialog__detail">
    <div className="model-library-dialog__detail-head">
      <div className="model-library-dialog__detail-title"><span className="model-library-dialog__detail-name">{t("models.addSourceTitle")}</span></div>
      <p className="model-library-dialog__hint">{t(sources.length > 0 ? "models.addSourceHint" : "models.addSourceHintFirst")}</p>
      <label className="provider-dialog__field">
        <span className="provider-dialog__label">{t("creds.provider")}</span>
        <select className="provider-dialog__select" value={preset} onChange={e => { setPreset(e.target.value); setError(""); }} disabled={saving}>
          {BRIDGE_PRESETS.map(p => <option key={p.id} value={p.id}>{p.label}</option>)}
        </select>
      </label>
      <label className="provider-dialog__field">
        <span className="provider-dialog__label">{t("creds.nameOptional")}</span>
        <input className="provider-dialog__input" type="text" value={label} onChange={e => setLabel(e.target.value)} placeholder={t("creds.labelPlaceholder")} disabled={saving} />
      </label>
      <label className="provider-dialog__field">
        <span className="provider-dialog__label">{t("v2dialogs.apiKey")}</span>
        <input className="provider-dialog__input" type="password" value={apiKey} onChange={e => setApiKey(e.target.value)} placeholder={t("creds.apiKeyPlaceholder")} disabled={saving} autoComplete="new-password" />
      </label>
      <div className="model-library-dialog__filters">
        <button type="button" className="model-library-dialog__btn model-library-dialog__btn--primary" disabled={saving} onClick={() => void add()}>{saving ? t("creds.saving") : t("creds.saveApi")}</button>
        <button type="button" className="model-library-dialog__text-btn" disabled={saving} onClick={onCustom}>{t("creds.csAdd")} <ChevronRight size={12} aria-hidden /></button>
      </div>
    </div>
    {error && <p role="alert" className="model-library-dialog__error">{error}</p>}
  </div>;
}

/** 一个来源的凭据块：打码密钥、检测、编辑（自定义）/删除（API Key）。 */
function SourceCredential({ label, entry, source, check, busy, confirming, deleting, onCheck, onEdit, onDelete }: {
  label?: string;
  entry: BridgeEntryView | null; source: ModelSource; check?: { state: string; detail?: string };
  busy: boolean; confirming: string | null; deleting: string | null;
  onCheck: () => void; onEdit: () => void; onDelete: (entry: BridgeEntryView) => void;
}) {
  const t = useT();
  return <div className="model-library-dialog__cred">
    {label ? <span className="model-library-dialog__cred-source">{label}</span> : null}
    <span className="model-library-dialog__faint">{t("models.credTitle")}</span>
    <code>{entry?.maskedKey || (entry ? t("creds.statusSavedLogin") : t("models.credScanned"))}</code>
    <span className="model-library-dialog__faint" title={check?.detail || source.issue || undefined}>{check?.state || t("creds.statusUnchecked")}</span>
    <button type="button" className="model-library-dialog__text-btn" disabled={busy || check?.state === t("creds.checking")} onClick={onCheck}>{t("creds.check")}</button>
    {entry?.preset === "custom" && <button type="button" className="model-library-dialog__text-btn" disabled={busy} onClick={onEdit}>{t("creds.csEdit")}</button>}
    {entry?.kind === "key-plan" && <button type="button" className={`model-library-dialog__text-btn${confirming === entry.id ? " is-danger" : ""}`} disabled={busy} onClick={() => onDelete(entry)}>{deleting === entry.id ? t("creds.deleting") : confirming === entry.id ? t("creds.confirmDelete") : t("common.delete")}</button>}
  </div>;
}

/** t() 的键是字面量类型——四族名映射成静态键，别拼模板串。 */
const FAMILY_LABEL_KEY = {
  claude: "models.family_claude",
  codex: "models.family_codex",
  grok: "models.family_grok",
  gemini: "models.family_gemini",
} as const;

const FAMILY_LOGIN_KEY = {
  claude: "models.shareFamilyNoLogin_claude",
  codex: "models.shareFamilyNoLogin_codex",
  grok: "models.shareFamilyNoLogin_grok",
  gemini: "models.shareFamilyNoLogin_gemini",
} as const;

const FAMILY_LOGIN_HOW_KEY = {
  claude: "models.familyLoginHow_claude",
  codex: "models.familyLoginHow_codex",
  grok: "models.familyLoginHow_grok",
  gemini: "models.familyLoginHow_gemini",
} as const;

function isShown(group: LibraryGroup, row: LibraryRow, prefs: ModelLibraryPreferences): boolean {
  return !prefs.hiddenProviders.includes(group.key) && !rowHidden(prefs, row);
}
function sourceOf(group: LibraryGroup, sources: ModelSource[], aliases: Record<string, string>): ModelSource | undefined {
  return sources.find(source => (aliases[`engine:${source.id}`] ?? `engine:${source.id}`) === group.key);
}
/** 合并展示里某一组叫什么：agent 自带目录 = "<agent> 的模型"；引擎来源 = 来源名。 */
function memberLabel(group: LibraryGroup, sources: ModelSource[], aliases: Record<string, string>, agents: AgentConfig[], t: Translate): string {
  if (group.key.startsWith("native:")) {
    const agentId = group.key.split(":")[1] ?? "";
    const agent = agents.find(candidate => candidate.id === agentId);
    return agent ? t("models.memberAgent", { agent: agent.label }) : group.label;
  }
  const source = sourceOf(group, sources, aliases);
  return source ? source.label : group.label;
}

/**
 * 一个来源的凭据条目：自定义来源按 origin 反查（`Marionette · …#<条目 id>`）；
 * 扫描来的 grok 通道按 channel 后缀对上。桥注入的 `grok/channel/*` 不算用户
 * 凭据——它们指向我们自己的引擎，没有可删的 key（2026-11-26 凭据页不再逐模型列）。
 */
function sourceForEntry(source: ModelSource, entry: BridgeEntryView, aliases: Record<string, string>): boolean {
  const canonical = aliases[`entry:${entry.id}`];
  if (canonical && canonical === (aliases[`engine:${source.id}`] || `engine:${source.id}`)) return true;
  const channel = entry.id.startsWith("grok/channel/") ? entry.id.slice("grok/channel/".length) : null;
  return channel !== null && !channel.startsWith("marionette") && source.origins.some(origin => origin.startsWith("grok-build · ") && origin.endsWith(`#${channel}`));
}
function entryForSource(source: ModelSource, entries: BridgeEntryView[], aliases: Record<string, string>): BridgeEntryView | null {
  return entries.find(entry => entry.kind !== "reference" && sourceForEntry(source, entry, aliases)) ?? null;
}

function agentLabel(agents: AgentConfig[], agentId: string): string {
  return agents.find(agent => agent.id === agentId)?.label ?? agentId;
}
function scanStatusText(source: BridgeScanSource, entries: BridgeEntryView[], t: Translate): string {
  if (source.status === "found") {
    const count = entries.filter(entry => entry.source === `scan:${source.agent}`).length;
    return count > 0 ? t("creds.scanFoundSome", { count }) : t("creds.scanFound");
  }
  if (source.status === "restricted") return t("creds.scanRestricted");
  return t("creds.scanMissing");
}
/** 一行里的引擎状态（未启动 / 运行中 · 端口 / 失败原因）。 */
function engineStatusText(engine: EngineStatus, t: Translate): string {
  const mb = (n: number) => `${(n / 1024 / 1024).toFixed(1)} MB`;
  switch (engine.state) {
    case "downloading":
      return engine.total > 0 ? t("creds.engineDownloadingPct", { pct: Math.round((engine.downloaded / engine.total) * 100), got: mb(engine.downloaded), total: mb(engine.total) }) : t("creds.engineDownloading", { got: mb(engine.downloaded) });
    case "starting": return t("creds.engineStarting");
    case "running": return engine.reused ? t("creds.engineRunningReused", { port: engine.port, version: engine.version ? ` · ${engine.version}` : "" }) : t("creds.engineRunning", { port: engine.port, version: engine.version ? ` · ${engine.version}` : "" });
    case "failed": return t("creds.engineFailed", { error: engine.error });
    default: return t("creds.engineMissing");
  }
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1)}M`;
  if (n >= 1000) return `${Math.round(n / 1000)}K`;
  return String(n);
}

/** Strip the plan suffix and punctuation so `Grok (SuperGrok)` ≡ `grok`. */
function plain(value: string | undefined | null): string {
  return (value ?? "").replace(/\s*\([^()]*\)\s*$/, "").toLowerCase().replace(/[^a-z0-9]/g, "");
}

function usedOf(w: EngineQuotaWindow): number | null {
  if (typeof w.used === "number") return Math.max(0, Math.min(100, w.used));
  if (typeof w.remaining === "number") return Math.max(0, Math.min(100, 100 - w.remaining));
  return null;
}

/** Same bands as the rail quota dials (pctLevel): yellow from 75 used, red from 90. */
function quotaTone(used: number | null): string {
  if (used == null) return "";
  const level = pctLevel(used);
  return level === "hot" ? "is-hot" : level === "warn" ? "is-warm" : "";
}

type SourceQuota = { windows: { w: EngineQuotaWindow; used: number }[]; balance: string | null };

function quotaWindows(entries: EngineQuotaEntry[], names: Set<string>): SourceQuota | null {
  if (entries.length === 0) return null;
  const entry = entries.find(e => names.has(plain(e.provider)) || names.has(plain(e.name)) || e.aliases?.some(id => names.has(plain(id))));
  if (!entry) return null;
  const windows = entry.windows.map(w => ({ w, used: usedOf(w) })).filter((x): x is { w: EngineQuotaWindow; used: number } => x.used != null).sort((a, b) => b.used - a.used);
  return { windows, balance: entry.balance?.trim() || null };
}

/** 订阅族 → 额度行：引擎里这一族的 provider id（gemini 族 = antigravity）。 */
function quotaForFamily(family: SubscriptionFamily, entries: EngineQuotaEntry[]): SourceQuota | null {
  const names = new Set<string>();
  const add = (value: string) => { const base = plain(value); if (base) names.add(base); };
  add(family);
  if (family === "gemini") { add("antigravity"); add("gemini"); }
  if (family === "codex") add("chatgpt");
  return quotaWindows(entries, names);
}

/**
 * The engine's quota rows are keyed by magpie provider id, the model library
 * by discovered source. The two only meet by name, so match on identity
 * spellings (id, host label, the vendor an agent lists it under) and never
 * guess further — a wrong quota is worse than none.
 */
function quotaFor(group: LibraryGroup, source: ModelSource | undefined, entries: EngineQuotaEntry[]): SourceQuota | null {
  const names = new Set<string>();
  const add = (value: string | undefined) => {
    const base = plain(value); if (base) names.add(base);
    const host = value && value.includes(".") ? plain(value.split(".")[0]) : ""; if (host) names.add(host);
  };
  add(group.label); add(source?.id); add(source?.label);
  for (const refs of Object.values(source?.nativeAccounts ?? {})) for (const ref of refs) { add(ref.provider); add(ref.label); }
  if (group.key.startsWith("engine:")) add(group.key.slice("engine:".length));
  if (group.key.startsWith("entry:")) add(group.label);
  return quotaWindows(entries, names);
}

/** Left-column verdict: tightest window, else balance, else a dash. */
function summarizeQuota(quota: SourceQuota | null, t: Translate): { label: string; tone: string } {
  if (!quota) return { label: "—", tone: "is-none" };
  const tight = quota.windows[0];
  if (tight) return { label: tight.used >= 100 ? t("models.quotaFull") : t("models.quotaLeft", { pct: Math.round(100 - tight.used) }), tone: quotaTone(tight.used) };
  if (quota.balance) return { label: quota.balance, tone: "" };
  return { label: "—", tone: "is-none" };
}

function quotaResetNote(w: EngineQuotaWindow, t: Translate): string {
  if (!w.resetsAt) return "";
  const left = Date.parse(w.resetsAt) - Date.now();
  if (!Number.isFinite(left) || left <= 0) return "";
  const hr = Math.floor(left / 3_600_000);
  const when = hr < 1 ? t("dock.resetMinutes", { n: Math.max(1, Math.round(left / 60_000)) })
    : hr < 48 ? t("dock.resetHours", { n: hr })
    : t("dock.resetDays", { n: Math.round(hr / 24) });
  return `${w.name} · ${when}`;
}

/**
 * 来源是「从哪发现」的（扫描的 origins："opencode · path#route"）。共享是全局的，
 * 没有「借给谁」这回事（2026-11-27 用户定案），这里只回答「这是什么、哪来的」。
 * 从凭据库来的写「本机凭据」；找得到对应 agent 就用它的名字与身份色。
 */
function originAgents(source: ModelSource, agents: AgentConfig[], t: Translate): { name: string; id: string | null }[] {
  const names = [...new Set(source.origins.map(o => o.split(" · ")[0]?.trim() ?? "").filter(Boolean))];
  const out: { name: string; id: string | null }[] = [];
  for (const name of names) {
    if (name === "Marionette") {
      out.push({ name: t("models.fromCredentials"), id: null });
      continue;
    }
    const n = plain(name);
    const agent = agents.find(a => plain(a.id) === n || plain(a.label) === n) ?? agents.find(a => plain(a.id).startsWith(n) || plain(a.label).startsWith(n));
    const entry = { name: agent?.label ?? name, id: agent?.id ?? null };
    if (!out.some(o => o.name === entry.name)) out.push(entry);
  }
  return out;
}

/**
 * A discovered source labels its rows `Vendor/Model`, and the group header
 * already names that vendor — often as the bare host (`ollama.com` for
 * `Ollama Cloud/…`). Compare the leading word so both spellings match, and
 * leave a name alone when the vendor word is genuinely part of the model.
 */
function rowLabel(groupLabel: string, label: string, modelId: string): string {
  const name = (label || modelId).trim();
  const slash = name.indexOf("/");
  if (slash <= 0) return name;
  const lead = (value: string) => value.trim().toLowerCase().match(/^[a-z0-9]+/)?.[0] ?? "";
  const vendor = lead(name.slice(0, slash));
  if (vendor.length < 3 || vendor !== lead(groupLabel)) return name;
  return name.slice(slash + 1).trim() || name;
}

function SourceEnvironmentInput({ sourceId, name, onSaved }: { sourceId: string; name: string; onSaved: () => Promise<void> }) {
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const t = useT();
  async function save() {
    setBusy(true); setError("");
    try { await configureSourceEnvironment(sourceId, name, value); setValue(""); await onSaved(); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return <div className="model-library-dialog__environment">
    <label><span>{name}</span><input type="password" autoComplete="new-password" aria-label={t("models.envVar", { name })} placeholder={t("models.envVarPlaceholder")} value={value} disabled={busy} onChange={e => setValue(e.target.value)} /><button type="button" className="model-library-dialog__text-btn is-accent" disabled={busy || !value.trim()} onClick={() => void save()}>{busy ? t("creds.saving") : t("models.saveAndApply")}</button></label>
    <small>{t("models.envVarNote")}</small>
    {error && <div role="alert" className="model-library-dialog__error">{error}</div>}
  </div>;
}
