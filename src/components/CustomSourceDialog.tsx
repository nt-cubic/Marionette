import { useState } from "react";
import { RefreshCw, X } from "lucide-react";
import { bridgeAddCustomProvider, bridgeUpdateCustomProvider, probeCustomSource, type ProbedModel } from "../lib/api";
import { invalidateBridgeModels } from "../lib/bridge";
import { refreshModelSources } from "../lib/modelLibrary";
import { useT } from "../lib/i18n";
import { DialogShell } from "./DialogShell";

/** The wire formats a custom source can speak (ids are `CustomProviderConfig.protocol`). */
const PROTOCOLS = [
  { id: "chat", label: "OpenAI Chat Completions" },
  { id: "responses", label: "OpenAI Responses" },
  { id: "anthropic", label: "Anthropic Messages" },
] as const;

type Protocol = (typeof PROTOCOLS)[number]["id"];

/**
 * A saved custom source as the edit entry point hands it over. The key is not
 * part of it: plaintext never leaves the store, so an edit keeps it unless the
 * user types a new one.
 */
export type CustomSourceDraft = {
  entryId: string;
  label: string;
  endpoint: string;
  protocol: Protocol;
  /** The fixed model list; empty means the source follows the service's live catalogue. */
  models: string[];
};

/** `label` is the service's own display name; empty when it gave none. */
type Row = { id: string; label: string };

const hostOf = (endpoint: string): string => {
  try {
    return new URL(endpoint.trim()).host;
  } catch {
    return "";
  }
};

/**
 * Add or edit a custom API source, in the order people think of it: endpoint →
 * protocol → key → "Fetch models" → save (2026-11-26; the old inline form made
 * model IDs be typed by hand and offered no edit at all).
 *
 * What the list shows is what gets saved:
 * - after a successful pull the source is *live* — nothing is pinned and the
 *   service's catalogue is read again whenever a menu asks;
 * - adding an ID by hand, or removing a row, turns it into a *fixed* list, which
 *   is what a service without a catalogue endpoint needs.
 */
export function CustomSourceDialog({
  initial = null,
  onClose,
  onSaved,
}: {
  /** Present = edit this saved source instead of adding one. */
  initial?: CustomSourceDraft | null;
  onClose: () => void;
  /** Runs once the store changed, before the dialog closes; the parent reloads its lists. */
  onSaved?: () => void | Promise<void>;
}) {
  const t = useT();
  const editing = initial !== null;
  const [endpoint, setEndpoint] = useState(initial?.endpoint ?? "");
  const [protocol, setProtocol] = useState<Protocol>(initial?.protocol ?? "chat");
  const [apiKey, setApiKey] = useState("");
  const [label, setLabel] = useState(initial?.label ?? "");
  /** null = live (follow the service's catalogue); a list = exactly these models. */
  const [rows, setRows] = useState<Row[] | null>(
    initial?.models.length ? initial.models.map(id => ({ id, label: "" })) : null,
  );
  /** The last successful pull for the current inputs; any edit to them drops it. */
  const [fetched, setFetched] = useState<ProbedModel[] | null>(null);
  /** IDs typed in this session. A re-pull keeps the ones the service does not list. */
  const [handAdded, setHandAdded] = useState<string[]>([]);
  const [rowsTouched, setRowsTouched] = useState(false);
  const [pulling, setPulling] = useState(false);
  const [failure, setFailure] = useState("");
  const [manual, setManual] = useState("");
  const [query, setQuery] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");

  // An edit that changed nothing about how the catalogue is reached may save
  // without a pull: the stored key is not shown, so a pull could not authenticate.
  const untouched =
    editing &&
    endpoint.trim() === initial.endpoint &&
    protocol === initial.protocol &&
    apiKey.trim() === "" &&
    !rowsTouched;

  const shownRows = rows ?? fetched ?? [];
  const q = query.trim().toLowerCase();
  const shown = q ? shownRows.filter(row => `${row.id} ${row.label}`.toLowerCase().includes(q)) : shownRows;
  const ready = rows ? rows.length > 0 : fetched !== null || untouched;

  /** Endpoint / protocol / key changed: the pull on screen no longer describes them. */
  const forgetPull = () => {
    setFetched(null);
    setFailure("");
  };

  async function pull() {
    const url = endpoint.trim();
    setError("");
    if (!url) {
      setError(t("creds.csNeedEndpoint"));
      return;
    }
    setPulling(true);
    setFailure("");
    try {
      const probe = await probeCustomSource(url, protocol, apiKey.trim());
      if (!probe.ok) {
        setFetched(null);
        setFailure(probe.detail);
        return;
      }
      setFetched(probe.models);
      // A pull that answers speaks for the whole list — except what was typed by
      // hand and the service does not list: typed input is never dropped silently.
      const listed = new Set(probe.models.map(model => model.id));
      const extras = handAdded.filter(id => !listed.has(id));
      setRows(extras.length ? [...probe.models, ...extras.map(id => ({ id, label: "" }))] : null);
    } catch (e) {
      setFetched(null);
      setFailure(e instanceof Error ? e.message : String(e));
    } finally {
      setPulling(false);
    }
  }

  function addManual() {
    // Comma is the engine's list separator, so it also separates IDs here.
    const ids = manual.split(/[,\s]+/).map(s => s.trim()).filter(Boolean);
    if (ids.length === 0) return;
    const known = new Set(shownRows.map(row => row.id));
    const fresh = ids.filter(id => !known.has(id));
    if (fresh.length > 0) {
      setRows([...shownRows, ...fresh.map(id => ({ id, label: "" }))]);
      setHandAdded(prev => [...prev, ...fresh]);
    }
    setRowsTouched(true);
    setManual("");
  }

  function removeRow(id: string) {
    const next = shownRows.filter(row => row.id !== id);
    // Removing the last row of a fixed list goes back to the service's catalogue
    // when there is one; without a pull nothing is left to save and `ready` says so.
    setRows(next.length ? next : null);
    setHandAdded(prev => prev.filter(x => x !== id));
    setRowsTouched(true);
  }

  async function save() {
    const url = endpoint.trim();
    setError("");
    if (!url) {
      setError(t("creds.csNeedEndpoint"));
      return;
    }
    if (!ready) {
      setError(t("creds.csNeedModels"));
      return;
    }
    const models = rows?.map(row => row.id) ?? [];
    const name = label.trim() || hostOf(url);
    setSaving(true);
    try {
      if (initial) {
        await bridgeUpdateCustomProvider(initial.entryId, name, apiKey.trim() || null, { endpoint: url, protocol, models });
      } else {
        await bridgeAddCustomProvider(name, apiKey.trim(), url, protocol, models);
      }
      invalidateBridgeModels();
      await refreshModelSources().catch(() => undefined);
      await onSaved?.();
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  }

  const busy = saving || pulling;
  const listEmpty = shownRows.length === 0;

  return (
    <DialogShell
      className="project-dialog-backdrop"
      dialogClassName="project-dialog custom-source-dialog"
      labelledBy="custom-source-title"
      onClose={() => {
        if (!busy) onClose();
      }}
    >
      <div className="project-dialog__header">
        <div>
          <strong id="custom-source-title">{t(editing ? "creds.csTitleEdit" : "creds.csTitleAdd")}</strong>
        </div>
        <button type="button" className="project-dialog__close" aria-label={t("common.close")} disabled={busy} onClick={onClose}>
          ×
        </button>
      </div>

      <label className="project-dialog__field">
        <span>{t("creds.endpoint")}</span>
        <input
          type="url"
          spellCheck={false}
          autoComplete="off"
          placeholder={t("creds.csEndpointPlaceholder")}
          value={endpoint}
          disabled={busy}
          autoFocus={!editing}
          onChange={event => {
            setEndpoint(event.target.value);
            forgetPull();
          }}
          onKeyDown={event => {
            if (event.key === "Enter") void pull();
          }}
        />
      </label>
      <label className="project-dialog__field">
        <span>{t("creds.protocol")}</span>
        <select value={protocol} disabled={busy} onChange={event => {
          setProtocol(event.target.value as Protocol);
          forgetPull();
        }}>
          {PROTOCOLS.map(option => <option key={option.id} value={option.id}>{option.label}</option>)}
        </select>
      </label>
      <label className="project-dialog__field">
        <span>{t("v2dialogs.apiKey")}</span>
        <input
          type="password"
          autoComplete="new-password"
          spellCheck={false}
          placeholder={t("creds.csKeyPlaceholder")}
          value={apiKey}
          disabled={busy}
          onChange={event => {
            setApiKey(event.target.value);
            forgetPull();
          }}
          onKeyDown={event => {
            if (event.key === "Enter") void pull();
          }}
        />
      </label>
      <label className="project-dialog__field">
        <span>{t("creds.sourceName")}</span>
        <input
          type="text"
          spellCheck={false}
          placeholder={t("creds.csNamePlaceholder")}
          value={label}
          disabled={busy}
          onChange={event => setLabel(event.target.value)}
        />
      </label>

      {editing && <p className="custom-source-dialog__note">{t("creds.csKeyKeepHint")}</p>}

      <div className="custom-source-dialog__models">
        <div className="custom-source-dialog__models-head">
          <span>{t("creds.csModels", { count: shownRows.length })}</span>
          <button
            type="button"
            className="custom-source-dialog__fetch"
            disabled={busy || !endpoint.trim()}
            onClick={() => void pull()}
          >
            <RefreshCw size={12} aria-hidden className={pulling ? "is-spinning" : undefined} />
            {pulling ? t("creds.csFetching") : t("creds.csFetch")}
          </button>
        </div>
        {shownRows.length > 1 && (
          <input
            className="custom-source-dialog__search"
            type="search"
            spellCheck={false}
            placeholder={t("creds.csSearch")}
            aria-label={t("creds.csSearch")}
            value={query}
            onChange={event => setQuery(event.target.value)}
          />
        )}
        <div className="custom-source-dialog__list" role={shown.length > 0 ? "list" : undefined}>
          {failure && (
            <p className="custom-source-dialog__failure" role="alert">
              {failure}
              <span className="custom-source-dialog__failure-tail">{t("creds.csFailedTail")}</span>
            </p>
          )}
          {!failure && listEmpty && !pulling && (
            <p className="custom-source-dialog__empty">{t("creds.csIdle")}</p>
          )}
          {q && shownRows.length > 0 && shown.length === 0 && (
            <p className="custom-source-dialog__empty">{t("creds.csNoMatch")}</p>
          )}
          {shown.map(row => (
            <div className="custom-source-dialog__row" role="listitem" key={row.id}>
              <span className="custom-source-dialog__id" title={row.id}>{row.id}</span>
              {row.label && <span className="custom-source-dialog__label">{row.label}</span>}
              {rows && (
                <button
                  type="button"
                  className="custom-source-dialog__remove"
                  aria-label={`${t("creds.csRemove")} ${row.id}`}
                  title={t("creds.csRemove")}
                  disabled={busy}
                  onClick={() => removeRow(row.id)}
                >
                  <X size={12} />
                </button>
              )}
            </div>
          ))}
        </div>
        <div className="custom-source-dialog__manual">
          <input
            type="text"
            spellCheck={false}
            autoComplete="off"
            placeholder={t("creds.csManualPlaceholder")}
            aria-label={t("creds.csManualPlaceholder")}
            value={manual}
            disabled={busy}
            onChange={event => setManual(event.target.value)}
            onKeyDown={event => {
              if (event.key === "Enter") {
                event.preventDefault();
                addManual();
              }
            }}
          />
        </div>
      </div>

      <p className="custom-source-dialog__note">
        {rows
          ? t("creds.csFixedNote", { count: rows.length })
          : fetched || editing
            ? t("creds.csLiveNote")
            : t("creds.csEndpointNote")}
      </p>
      {error && <p className="project-dialog__error" role="alert">{error}</p>}

      <div className="project-dialog__actions">
        <button type="button" className="project-dialog__cancel" disabled={busy} onClick={onClose}>{t("common.cancel")}</button>
        <button type="button" className="project-dialog__submit" disabled={busy || !ready} onClick={() => void save()}>
          {saving ? t("creds.saving") : t("common.save")}
        </button>
      </div>
    </DialogShell>
  );
}
