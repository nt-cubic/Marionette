import { useEffect, useMemo, useState } from "react";
import { ArrowDown, ArrowUp, ChevronRight, RefreshCw, X } from "lucide-react";
import { bridgeListModels, type BridgeModelsResult } from "../lib/api";
import { cachedCapabilitiesFor } from "../lib/capabilityCache";
import { invalidateBridgeModels } from "../lib/bridge";
import { configureSourceEnvironment, expandHiddenProviders, DEFAULT_MODEL_PREFERENCES, libraryGroups, ordered, refreshModelSources, rowHidden, rowKeys, saveModelPreferences, toggleRow, useModelLibrary, type LibraryGroup, type ModelLibraryPreferences } from "../lib/modelLibrary";
import { splitModelMeta } from "../lib/modelMenu";
import { useT } from "../lib/i18n";
import type { AgentConfig } from "../lib/types";

export function ModelLibraryDialog({ agents, onClose, onManageSources }: { agents: AgentConfig[]; onClose: () => void; onManageSources: () => void }) {
  const { preferences, sources, warnings, aliases, accounts, error: loadError } = useModelLibrary(false);
  const [draft, setDraft] = useState(preferences);
  const [catalog, setCatalog] = useState<BridgeModelsResult | null>(null);
  const [query, setQuery] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  /** Groups the user opened or closed; unset groups follow the size rule. */
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const t = useT();
  useEffect(() => { if (!dirty) setDraft(preferences); }, [preferences, dirty]);
  async function refresh() {
    setBusy(true); setError("");
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
  // Closing stays possible while busy: the opening refresh can take a while
  // (source scans + registrations), and a dialog that can't be dismissed
  // reads as a hang. The fetch keeps running; a setState after unmount is a
  // harmless no-op in React 18.
  return <div className="project-dialog-backdrop" onClick={onClose}>
    <div className="project-dialog model-library-dialog" role="dialog" aria-modal="true" aria-label={t("models.dialogLabel")} onClick={e => e.stopPropagation()}>
      <div className="project-dialog__header model-library-dialog__header"><span>{t("models.dialogLabel")}</span><button type="button" className="pill-action pill-action--icon" aria-label={t("common.close")} onClick={onClose}><X size={16} /></button></div>
      <div className="project-dialog__body model-library-dialog__body">
        <p className="model-library-dialog__hint">{t("models.hint")}</p>
        <div className="model-library-dialog__toolbar"><input type="search" placeholder={t("models.searchPlaceholder")} aria-label={t("models.searchPlaceholder")} value={query} onChange={e => setQuery(e.target.value)} /><button type="button" className="project-dialog__cancel" disabled={busy || dirty} title={dirty ? t("models.saveFirst") : t("models.manageSourcesTitle")} onClick={onManageSources}>{t("models.manageSources")}</button><button type="button" className="project-dialog__cancel" disabled={busy} aria-label={t("models.rescan")} title={t("models.rescan")} onClick={() => void refresh()}><RefreshCw size={14} /></button></div>
        {(error || loadError) && <p role="alert" className="model-library-dialog__error">{error || loadError}</p>}
        {warnings.length > 0 && <details className="model-library-dialog__warn">
          <summary><span className="model-library-dialog__warn-dot" aria-hidden />{warnings.every(isRegisterFailedWarning) ? t("models.sourcesFailed", { count: warnings.length }) : t("models.sourceNotices", { count: warnings.length })}<span className="model-library-dialog__warn-more">{t("models.view")}</span><button type="button" className="model-library-dialog__link" disabled={busy} onClick={e => { e.preventDefault(); e.stopPropagation(); void refresh(); }}>{busy ? t("creds.refreshing") : t("models.refreshRetry")}</button></summary>
          {warnings.map(w => <div key={w}>{w}</div>)}
        </details>}
        {catalog?.note && <p className="model-library-dialog__hint">{catalog.note}</p>}
        {filtered.length === 0 && <p className="model-library-dialog__hint">{busy ? t("models.readingCatalog") : t("models.noMatches")}</p>}
        {filtered.length > 0 && <div className="model-library-dialog__list">{filtered.map(group => {
          const source = sources.find(s => (aliases[`engine:${s.id}`] ?? `engine:${s.id}`) === group.key);
          const selected = group.rows.filter(r => !draft.hiddenProviders.includes(group.key) && !rowHidden(draft, r)).length;
          const hidden = selected === 0;
          const index = groups.findIndex(g => g.key === group.key);
          // Long accounts (a vendor catalogue of 20+ models) would bury every
          // other source under their rows. Small ones start open; the rest show
          // the selected models first and expand on demand. A search keeps every
          // match visible, since collapsing would hide what was asked for.
          const matched = group.rows.filter(r => !q || group.label.toLowerCase().includes(q) || `${r.label} ${r.modelId}`.toLowerCase().includes(q));
          const open = q ? true : expanded[group.key] ?? group.rows.length <= 8;
          // Collapsed, a long account shows the models it actually shares —
          // everything else is behind "显示全部", so one vendor catalogue of 20+
          // rows cannot bury the sources below it.
          const rowsInScope = q ? matched : group.rows;
          const shownRows = open ? rowsInScope : rowsInScope.filter(r => !rowHidden(draft, r)).slice(0, 3);
          const restCount = rowsInScope.length - shownRows.length;
          return <section key={group.key} className={`model-library-dialog__group${hidden ? " is-hidden" : ""}`}>
            <div className="model-library-dialog__provider">
              <button type="button" className="model-library-dialog__disclosure" aria-expanded={open} aria-label={open ? t("models.collapseSource", { name: group.label }) : t("models.expandSource", { name: group.label })} onClick={() => setExpanded(e => ({ ...e, [group.key]: !open }))}><ChevronRight size={14} className={open ? "is-open" : ""} /></button>
              <label><input type="checkbox" checked={group.rows.length > 0 && selected === group.rows.length} aria-checked={selected > 0 && selected < group.rows.length ? "mixed" : selected > 0} ref={el => { if (el) el.indeterminate = selected > 0 && selected < group.rows.length; }} disabled={!group.rows.length} onChange={e => {
                const next = expandHiddenProviders(draft, groups);
                // Group toggle works on current keys; stale pre-merge keys of a
                // shown row are cleared so they cannot hide it again on save.
                const keys = new Set(group.rows.flatMap(rowKeys));
                if (e.target.checked) change({ ...next, hiddenModels: next.hiddenModels.filter(key => !keys.has(key)) });
                else change({ ...next, hiddenModels: [...new Set([...next.hiddenModels, ...group.rows.map(r => r.key)])] });
              }} /> <strong>{group.label}</strong>
                {source && <span className="model-library-dialog__proto">{source.protocol}</span>}
                {source && [...new Set(source.origins.map(o => o.split(" · ")[0]?.trim()).filter(Boolean))].map(agent => <span key={agent} className="model-library-dialog__agent">{agent}</span>)}
              </label>
              <span className="model-library-dialog__count">{selected === group.rows.length ? t("models.countAll", { count: group.rows.length }) : `${selected}/${group.rows.length}`}{group.key.startsWith("native:") ? ` · ${group.key.split(":")[1]}` : ""}</span>
              <div className="model-library-dialog__order">
                <button type="button" aria-label={t("models.moveSourceUp", { name: group.label })} disabled={index === 0} onClick={() => change({ ...draft, providerOrder: move(groups.map(g => g.key), group.key, -1) })}><ArrowUp size={14} /></button>
                <button type="button" aria-label={t("models.moveSourceDown", { name: group.label })} disabled={index === groups.length - 1} onClick={() => change({ ...draft, providerOrder: move(groups.map(g => g.key), group.key, 1) })}><ArrowDown size={14} /></button>
              </div>
            </div>
            {open && source && <div className="model-library-dialog__origin" title={[source.endpoint, ...source.origins].join("\n")}><span>{source.endpoint}</span><span className="model-library-dialog__origin-files">{source.origins.map(o => o.split(" · ").slice(1).join(" · ").split(/[\\/]/).pop()).join(" · ")}</span></div>}
            {source && (source.issue || source.missingEnv?.length) ? <div className="model-library-dialog__issue">{source.issue && <div className="model-library-dialog__error">{source.issue}</div>}{source.missingEnv?.map(name => <SourceEnvironmentInput key={name} sourceId={source.id} name={name} onSaved={refresh} />)}</div> : null}
            {shownRows.length > 0 && <div className="model-library-dialog__models">{shownRows.map(row => {
              const rowIndex = group.rows.findIndex(r => r.key === row.key);
              const reorder = (delta: number) => change({ ...draft, modelOrder: { ...draft.modelOrder, [group.key]: move(group.rows.map(r => r.key), row.key, delta) } });
              return <div className="model-library-dialog__model" key={row.key}>
                <label><input type="checkbox" checked={!draft.hiddenProviders.includes(group.key) && !rowHidden(draft, row)} onChange={() => { const next = expandHiddenProviders(draft, groups); change(toggleRow(next, row)); }} />{(() => { const { name, meta } = splitModelMeta(rowLabel(group.label, row.label, row.modelId)); return <span className="model-library-dialog__name" title={row.modelId}>{name}{meta && <em>{meta}</em>}</span>; })()}<small>{row.modelId}</small></label>
                <div className="model-library-dialog__order">
                  <button type="button" aria-label={t("models.moveModelUp", { name: row.label })} disabled={rowIndex === 0} onClick={() => reorder(-1)}><ArrowUp size={13} /></button>
                  <button type="button" aria-label={t("models.moveModelDown", { name: row.label })} disabled={rowIndex === group.rows.length - 1} onClick={() => reorder(1)}><ArrowDown size={13} /></button>
                </div>
              </div>;
            })}</div>}
            {!open && restCount > 0 && <button type="button" className="model-library-dialog__more" onClick={() => setExpanded(e => ({ ...e, [group.key]: true }))}>{t("models.showAll", { count: rowsInScope.length })}</button>}
          </section>;
        })}</div>}
      </div>
      <div className="project-dialog__actions"><button type="button" className="project-dialog__cancel" disabled={busy} onClick={() => change(DEFAULT_MODEL_PREFERENCES)}>{t("models.resetAll")}</button><button type="button" className="project-dialog__cancel" onClick={onClose}>{t("common.cancel")}</button><button type="button" className="project-dialog__submit" disabled={busy} onClick={() => void save()}>{t("common.save")}</button></div>
    </div>
  </div>;
}

/**
 * A discovered source labels its rows `Vendor/Model`, and the group header
 * already prints the vendor. Drop that repetition so a row reads as the model.
 * A vendor whose own model name contains the vendor word is left alone.
 */
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
    <label>{name}<input type="password" autoComplete="new-password" aria-label={t("models.envVar", { name })} placeholder={t("models.envVarPlaceholder")} value={value} disabled={busy} onChange={e => setValue(e.target.value)} /></label>
    <button type="button" className="project-dialog__cancel" disabled={busy || !value.trim()} onClick={() => void save()}>{busy ? t("creds.saving") : t("models.saveAndApply")}</button>
    <small>{t("models.envVarNote")}</small>
    {error && <div role="alert" className="model-library-dialog__error">{error}</div>}
  </div>;
}
