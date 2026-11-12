import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, ChevronRight, Search, X } from "lucide-react";
import { bridgeListModels, engineQuota, type BridgeModelsResult, type EngineQuotaEntry, type EngineQuotaWindow } from "../lib/api";
import { agentInkStyle } from "../lib/agentInk";
import { cachedCapabilitiesFor } from "../lib/capabilityCache";
import { invalidateBridgeModels } from "../lib/bridge";
import { configureSourceEnvironment, expandHiddenProviders, DEFAULT_MODEL_PREFERENCES, libraryGroups, ordered, refreshModelSources, rowHidden, rowKeys, saveModelPreferences, toggleRow, useModelLibrary, type LibraryGroup, type LibraryRow, type ModelLibraryPreferences, type ModelSource } from "../lib/modelLibrary";
import { splitModelMeta } from "../lib/modelMenu";
import { pctLevel } from "../lib/quotaRail";
import { useT } from "../lib/i18n";
import type { AgentConfig } from "../lib/types";

type RowFilter = "all" | "on" | "off";
type Translate = ReturnType<typeof useT>;

export function ModelLibraryDialog({ agents, onClose, onManageSources }: { agents: AgentConfig[]; onClose: () => void; onManageSources: () => void }) {
  const { preferences, sources, warnings, aliases, accounts, error: loadError } = useModelLibrary(false);
  const [draft, setDraft] = useState(preferences);
  const [catalog, setCatalog] = useState<BridgeModelsResult | null>(null);
  const [quotas, setQuotas] = useState<EngineQuotaEntry[]>([]);
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [selectedKey, setSelectedKey] = useState("");
  const [filter, setFilter] = useState<RowFilter>("all");
  const t = useT();
  useEffect(() => { if (!dirty) setDraft(preferences); }, [preferences, dirty]);
  async function refresh() {
    setBusy(true); setError("");
    // Quotas are advisory and never gate the scan: the gateway answers from
    // its cache (`kept=1`) quickly, or not at all.
    void engineQuota(false).then(rows => { if (rows.length > 0) setQuotas(rows); }).catch(() => undefined);
    try {
      await refreshModelSources();
      invalidateBridgeModels();
      setCatalog(await bridgeListModels("grok-build"));
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  useEffect(() => { void refresh(); }, []);
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
    return ordered([...result.values()], draft.providerOrder, g => g.key, g => g.label)
      .map(g => ({ ...g, rows: ordered(g.rows, draft.modelOrder[g.key] ?? [], r => r.key, r => r.label) }));
  }, [agents, catalog, sources, aliases, accounts, draft.providerOrder, draft.modelOrder]);
  useEffect(() => {
    if (draft.hiddenProviders.some(key => groups.some(g => g.key === key))) {
      setDraft(expandHiddenProviders(draft, groups));
    }
  }, [draft, groups]);
  const q = query.trim().toLowerCase();
  const filtered = groups.filter(g => !q || `${g.label} ${g.rows.map(r => `${r.label} ${r.modelId}`).join(" ")}`.toLowerCase().includes(q));
  /** Backend warnings are generated per-locale at refresh time (engine.rs ts!):
   * match either wording so the summary stays right after a language switch. */
  const isRegisterFailedWarning = (w: string) => w.includes("注册失败") || w.includes("registration failed");
  const change = (next: ModelLibraryPreferences) => { setDraft(next); setDirty(true); };
  function move(keys: string[], key: string, delta: number): string[] {
    const next = keys.slice(); const from = next.indexOf(key); const to = from + delta;
    if (from >= 0 && to >= 0 && to < next.length) [next[from], next[to]] = [next[to]!, next[from]!];
    return next;
  }
  async function save() {
    setBusy(true); setError("");
    try { await saveModelPreferences(expandHiddenProviders(draft, groups)); onClose(); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  const isShown = (group: LibraryGroup, row: LibraryRow, prefs: ModelLibraryPreferences) => !prefs.hiddenProviders.includes(group.key) && !rowHidden(prefs, row);
  // Footer summary and the unsaved count both read straight off the rows, so
  // "3 处未保存" means three visibility flips (plus one for any order change).
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
  const current = filtered.find(g => g.key === selectedKey) ?? filtered[0];
  const sourceOf = (group: LibraryGroup) => sources.find(s => (aliases[`engine:${s.id}`] ?? `engine:${s.id}`) === group.key);
  const currentSource = current ? sourceOf(current) : undefined;
  const sharedIn = (group: LibraryGroup) => group.rows.filter(r => isShown(group, r, draft)).length;
  const currentQuota = current ? quotaFor(current, currentSource, quotas) : null;
  const currentShared = current ? sharedIn(current) : 0;
  const rowsInView = current ? current.rows.filter(r => {
    const on = isShown(current, r, draft);
    if (filter === "on" && !on) return false;
    if (filter === "off" && on) return false;
    return !q || current.label.toLowerCase().includes(q) || `${r.label} ${r.modelId}`.toLowerCase().includes(q);
  }) : [];
  const lentTo = current ? lentAgents(current, currentSource, agents) : null;
  const statusLine = busy ? t("models.scanning") : t("models.sourceCount", { count: groups.length });
  function toggleSource(group: LibraryGroup, on: boolean) {
    const next = expandHiddenProviders(draft, groups);
    // Group toggle works on current keys; stale pre-merge keys of a shown row
    // are cleared so they cannot hide it again on save.
    const keys = new Set(group.rows.flatMap(rowKeys));
    if (on) change({ ...next, hiddenModels: next.hiddenModels.filter(key => !keys.has(key)) });
    else change({ ...next, hiddenModels: [...new Set([...next.hiddenModels, ...group.rows.map(r => r.key)])] });
  }
  // Closing stays possible while busy: the opening refresh can take a while
  // (source scans + registrations), and a dialog that can't be dismissed
  // reads as a hang. The fetch keeps running; a setState after unmount is a
  // harmless no-op in React 18.
  return <div className="project-dialog-backdrop" onClick={onClose}>
    <div className="project-dialog model-library-dialog" role="dialog" aria-modal="true" aria-label={t("models.dialogLabel")} onClick={e => e.stopPropagation()}>
      <div className="model-library-dialog__header">
        <span className="model-library-dialog__title">{t("models.title")}</span>
        <span className="model-library-dialog__status">{statusLine}</span>
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
          <div className="model-library-dialog__source-list">
            {filtered.length === 0 && <p className="model-library-dialog__empty">{busy ? t("models.readingCatalog") : t("models.noMatches")}</p>}
            {filtered.map(group => {
              const source = sourceOf(group);
              const shared = sharedIn(group);
              const needKey = !!source?.missingEnv?.length;
              const sub = needKey ? { text: t("models.needKey"), warn: true }
                : source?.issue && group.rows.length === 0 ? { text: t("models.sourceIssue"), warn: true }
                : shared === 0 ? { text: t("models.unshared"), warn: false }
                : { text: `${t("models.shared")} ${shared} / ${group.rows.length}`, warn: false };
              const quota = summarizeQuota(quotaFor(group, source, quotas), t);
              return <button type="button" key={group.key} className={`model-library-dialog__source${group === current ? " is-selected" : ""}`} aria-current={group === current ? "true" : undefined} onClick={() => { setSelectedKey(group.key); setFilter("all"); }}>
                <span className="model-library-dialog__source-text"><span className="model-library-dialog__source-name">{group.label}</span><span className={`model-library-dialog__source-sub${sub.warn ? " is-warn" : ""}`}>{sub.text}</span></span>
                <span className={`model-library-dialog__source-quota ${quota.tone}`}>{quota.label}</span>
              </button>;
            })}
          </div>
          <button type="button" className="model-library-dialog__manage" disabled={busy || dirty} title={dirty ? t("models.saveFirst") : t("models.manageSourcesTitle")} onClick={onManageSources}>{t("models.manageSourcesLink")} <ChevronRight size={13} aria-hidden /></button>
        </div>
        {current ? <div className="model-library-dialog__detail">
          <div className="model-library-dialog__detail-head">
            <div className="model-library-dialog__detail-title">
              <span className="model-library-dialog__detail-name">{current.label}</span>
              {currentSource && <span className="model-library-dialog__faint">{currentSource.protocol}</span>}
              {!currentSource && current.key.startsWith("native:") && <span className="model-library-dialog__faint">{current.key.split(":")[1]}</span>}
              <span className="model-library-dialog__spacer" />
              <span className="model-library-dialog__faint">{t("models.shareSource")}</span>
              <button type="button" role="switch" className="model-library-dialog__switch" aria-checked={currentShared > 0} aria-label={t("models.shareSource")} disabled={!current.rows.length} onClick={() => toggleSource(current, currentShared === 0)}><i /></button>
            </div>
            {lentTo && <div className="model-library-dialog__lend">
              <span className="model-library-dialog__faint">{t("models.lendTo")}</span>
              {lentTo.length === 0 ? <span className="model-library-dialog__faint">{t("models.noAgents")}</span>
                : lentTo.map(a => <span key={a.name} className="model-library-dialog__agent" style={agentInkStyle(a.id)}>{a.name}</span>)}
              {currentSource && <span className="model-library-dialog__endpoint" title={[currentSource.endpoint, ...currentSource.origins].join("\n")}>{currentSource.endpoint}</span>}
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
            {currentSource && (currentSource.issue || currentSource.missingEnv?.length) ? <div className="model-library-dialog__issue">
              {currentSource.issue && <div className="model-library-dialog__issue-text">{currentSource.issue}</div>}
              {currentSource.missingEnv?.map(name => <SourceEnvironmentInput key={name} sourceId={currentSource.id} name={name} onSaved={refresh} />)}
            </div> : null}
            <div className="model-library-dialog__filters" role="group">
              {([["all", t("models.all"), current.rows.length], ["on", t("models.shared"), currentShared], ["off", t("models.unshared"), current.rows.length - currentShared]] as const).map(([id, label, n]) =>
                <button type="button" key={id} className={`model-library-dialog__chip${filter === id ? " is-active" : ""}`} aria-pressed={filter === id} onClick={() => setFilter(id)}>{label} {n}</button>)}
            </div>
          </div>
          <div className="model-library-dialog__models">
            {rowsInView.length === 0 && <p className="model-library-dialog__empty">{t("models.noModelsInView")}</p>}
            {rowsInView.map(row => {
              const rowIndex = current.rows.findIndex(r => r.key === row.key);
              const on = isShown(current, row, draft);
              const reorder = (delta: number) => change({ ...draft, modelOrder: { ...draft.modelOrder, [current.key]: move(current.rows.map(r => r.key), row.key, delta) } });
              const { name, meta } = splitModelMeta(rowLabel(current.label, row.label, row.modelId));
              return <div className={`model-library-dialog__model${on ? " is-on" : ""}`} key={row.key}>
                <label>
                  <input type="checkbox" checked={on} onChange={() => { const next = expandHiddenProviders(draft, groups); change(toggleRow(next, row)); }} />
                  <span className="model-library-dialog__name" title={row.modelId}>{name}{meta && <em>{meta}</em>}</span>
                  <small>{row.modelId}</small>
                </label>
                <div className="model-library-dialog__order">
                  <button type="button" aria-label={t("models.moveModelUp", { name: row.label })} disabled={rowIndex === 0} onClick={() => reorder(-1)}><ArrowUp size={13} /></button>
                  <button type="button" aria-label={t("models.moveModelDown", { name: row.label })} disabled={rowIndex === current.rows.length - 1} onClick={() => reorder(1)}><ArrowDown size={13} /></button>
                </div>
              </div>;
            })}
          </div>
        </div> : <div className="model-library-dialog__detail"><p className="model-library-dialog__empty">{busy ? t("models.readingCatalog") : t("models.noMatches")}</p></div>}
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
  </div>;
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

/**
 * The engine's quota rows are keyed by magpie provider id, the model library
 * by discovered source. The two only meet by name, so match on identity
 * spellings (id, host label, the vendor an agent lists it under) and never
 * guess further — a wrong quota is worse than none.
 */
function quotaFor(group: LibraryGroup, source: ModelSource | undefined, entries: EngineQuotaEntry[]): SourceQuota | null {
  if (entries.length === 0) return null;
  const names = new Set<string>();
  const add = (value: string | undefined) => {
    const base = plain(value); if (base) names.add(base);
    // `deepseek.com` registers as the vendor `deepseek`.
    const host = value && value.includes(".") ? plain(value.split(".")[0]) : ""; if (host) names.add(host);
  };
  add(group.label); add(source?.id); add(source?.label);
  for (const refs of Object.values(source?.nativeAccounts ?? {})) for (const ref of refs) { add(ref.provider); add(ref.label); }
  if (group.key.startsWith("engine:")) add(group.key.slice("engine:".length));
  const entry = entries.find(e => names.has(plain(e.provider)) || names.has(plain(e.name)));
  if (!entry) return null;
  const windows = entry.windows.map(w => ({ w, used: usedOf(w) })).filter((x): x is { w: EngineQuotaWindow; used: number } => x.used != null).sort((a, b) => b.used - a.used);
  return { windows, balance: entry.balance?.trim() || null };
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
 * Agents a source is lent to, from the scan's `origins` ("opencode · path#route").
 * `null` for groups with no such notion (credential entries). Origin names are
 * agent ids or short labels, so match both and keep the raw name when no
 * configured agent owns it (its ink falls back to the neutral one).
 */
function lentAgents(group: LibraryGroup, source: ModelSource | undefined, agents: AgentConfig[]): { name: string; id: string | null }[] | null {
  let names: string[];
  if (source) names = [...new Set(source.origins.map(o => o.split(" · ")[0]?.trim() ?? "").filter(Boolean))];
  else if (group.key.startsWith("native:")) names = [group.key.split(":")[1] ?? ""].filter(Boolean);
  else return null;
  const out: { name: string; id: string | null }[] = [];
  for (const name of names) {
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
