import { useCallback, useEffect, useMemo, useState } from "react";
import {
  addCustomAgent,
  fetchAcpRegistry,
  type AcpCatalogAgent,
  type CustomAgentDef,
} from "../lib/api";
import type { AgentConfig } from "../lib/types";
import { useT } from "../lib/i18n";

type CustomAgentDialogProps = {
  agents: AgentConfig[];
  customIds: Set<string>;
  onClose: () => void;
  onAdded: (note: string) => void;
};

function deriveCommand(pkg: string): string {
  let s = pkg.trim();
  for (const sep of ["==", ">=", "<=", "~=", "!=", ">", "<"]) {
    const idx = s.indexOf(sep);
    if (idx >= 0) s = s.slice(0, idx);
  }
  const bracket = s.indexOf("[");
  if (bracket >= 0) s = s.slice(0, bracket);
  const searchFrom = s.startsWith("@") ? (s.indexOf("/") + 1 || 0) : 0;
  const at = s.indexOf("@", searchFrom);
  if (at >= 0) s = s.slice(0, at);
  const slash = s.lastIndexOf("/");
  if (slash >= 0) s = s.slice(slash + 1);
  return s.trim();
}

function unpinnedPackage(spec: string): string {
  let s = spec.trim();
  const searchFrom = s.startsWith("@") ? (s.indexOf("/") + 1 || 0) : 0;
  const at = s.indexOf("@", searchFrom);
  if (at >= 0) s = s.slice(0, at);
  const eq = s.indexOf("==");
  if (eq >= 0) s = s.slice(0, eq);
  return s.trim();
}

function safeId(raw: string): string {
  const slug = raw
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
  return slug || "custom-agent";
}

export function CustomAgentDialog({
  agents,
  customIds,
  onClose,
  onAdded,
}: CustomAgentDialogProps) {
  const [tab, setTab] = useState<"registry" | "manual">("registry");
  const [query, setQuery] = useState("");
  const [catalog, setCatalog] = useState<AcpCatalogAgent[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [formError, setFormError] = useState<string | null>(null);
  const t = useT();
  const [manual, setManual] = useState({
    id: "custom-",
    label: "",
    command: "",
    args: "",
    npmPackage: "",
  });

  const builtinIds = useMemo(() => new Set(agents.map((a) => a.id)), [agents]);

  const load = useCallback(async () => {
    setLoadError(null);
    try {
      const list = await fetchAcpRegistry();
      setCatalog(list);
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e));
      setCatalog([]);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const filtered = useMemo(() => {
    const list = catalog ?? [];
    const q = query.trim().toLowerCase();
    if (!q) return list;
    return list.filter(
      (a) =>
        a.id.toLowerCase().includes(q) ||
        a.name.toLowerCase().includes(q) ||
        a.description.toLowerCase().includes(q),
    );
  }, [catalog, query]);

  const taken = (id: string) => builtinIds.has(id) || customIds.has(id);

  const addFromCatalog = async (entry: AcpCatalogAgent) => {
    const npx = entry.channels.find((c) => c.kind === "npx");
    const uvx = entry.channels.find((c) => c.kind === "uvx");
    const channel = npx ?? uvx;
    if (!channel?.package) {
      setFormError(
        t("customAgent.binaryOnly", { name: entry.name }),
      );
      return;
    }
    let id = safeId(entry.id);
    if (taken(id)) {
      if (builtinIds.has(id)) {
        setFormError(t("customAgent.builtinAlready", { name: entry.name, id }));
        return;
      }
      id = safeId(`custom-${entry.id}`);
    }
    if (taken(id)) {
      setFormError(t("customAgent.alreadyAdded", { name: entry.name }));
      return;
    }
    const cmd = (channel.cmd ?? "").trim() || deriveCommand(channel.package);
    const def: CustomAgentDef = {
      id,
      label: entry.name,
      command: cmd,
      args: channel.args ?? [],
      npmPackage: channel.kind === "npx" ? unpinnedPackage(channel.package) : null,
      note:
        channel.kind === "uvx"
          ? `uvx --from '${channel.package}' ${cmd}`
          : entry.description || null,
    };
    setBusyId(entry.id);
    setFormError(null);
    try {
      await addCustomAgent(def);
      onAdded(t("customAgent.addedNote", { name: entry.name }));
    } catch (e) {
      setFormError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyId(null);
    }
  };

  const addManual = async () => {
    const id = safeId(manual.id);
    if (!id || (taken(id) && builtinIds.has(id))) {
      setFormError(builtinIds.has(id) ? t("customAgent.idTaken", { id }) : t("customAgent.idInvalid"));
      return;
    }
    if (!manual.label.trim() || !manual.command.trim()) {
      setFormError(t("customAgent.needLabelAndCommand"));
      return;
    }
    const args = manual.args
      .split(/\s+/)
      .map((s) => s.trim())
      .filter(Boolean);
    setBusyId("manual");
    setFormError(null);
    try {
      await addCustomAgent({
        id,
        label: manual.label.trim(),
        command: manual.command.trim(),
        args,
        npmPackage: manual.npmPackage.trim() || null,
        note: null,
      });
      onAdded(t("customAgent.addedNote", { name: manual.label.trim() }));
    } catch (e) {
      setFormError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyId(null);
    }
  };

  return (
    <div className="project-dialog-backdrop" onClick={onClose}>
      <div
        className="project-dialog custom-agent-dialog"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-labelledby="custom-agent-title"
      >
        <h2 id="custom-agent-title">{t("customAgent.title")}</h2>
        <p className="custom-agent-dialog__lead">
          {t("customAgent.lead")}
        </p>
        <div className="custom-agent-dialog__tabs">
          <button
            type="button"
            className={tab === "registry" ? "is-selected" : ""}
            onClick={() => setTab("registry")}
          >
            {t("customAgent.tabRegistry")}
          </button>
          <button
            type="button"
            className={tab === "manual" ? "is-selected" : ""}
            onClick={() => setTab("manual")}
          >
            {t("customAgent.tabManual")}
          </button>
        </div>
        {tab === "registry" ? (
          <>
            <input
              className="custom-agent-dialog__search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder={t("customAgent.searchPlaceholder")}
              autoFocus
            />
            {loadError && (
              <p className="custom-agent-dialog__error">
                {loadError}{" "}
                <button type="button" onClick={() => void load()}>
                  {t("common.retry")}
                </button>
              </p>
            )}
            {formError && <p className="custom-agent-dialog__error">{formError}</p>}
            {!catalog && !loadError && <p className="custom-agent-dialog__hint">{t("customAgent.fetching")}</p>}
            <div className="custom-agent-dialog__list custom-scrollbar">
              {filtered.map((entry) => {
                const already = taken(safeId(entry.id)) || taken(safeId(`custom-${entry.id}`));
                const builtin = builtinIds.has(safeId(entry.id));
                return (
                  <div key={entry.id} className="custom-agent-dialog__row">
                    <div className="custom-agent-dialog__meta">
                      <strong>{entry.name}</strong>
                      <span>
                        {entry.id}
                        {entry.version ? ` · ${entry.version}` : ""}
                        {entry.installable ? "" : " · binary"}
                      </span>
                      {entry.description ? <em>{entry.description}</em> : null}
                    </div>
                    <button
                      type="button"
                      className="composer-agent-row__install"
                      disabled={Boolean(busyId) || builtin || already}
                      onClick={() => void addFromCatalog(entry)}
                    >
                      {busyId === entry.id
                        ? "…"
                        : builtin
                          ? t("customAgent.builtin")
                          : already
                            ? t("customAgent.added")
                            : entry.installable
                              ? t("customAgent.add")
                              : t("customAgent.cannotInstall")}
                    </button>
                  </div>
                );
              })}
            </div>
          </>
        ) : (
          <div className="custom-agent-dialog__form">
            {formError && <p className="custom-agent-dialog__error">{formError}</p>}
            <label>
              id
              <input
                value={manual.id}
                onChange={(e) => setManual((m) => ({ ...m, id: e.target.value }))}
              />
            </label>
            <label>
              {t("provider.displayName")}
              <input
                value={manual.label}
                onChange={(e) => setManual((m) => ({ ...m, label: e.target.value }))}
              />
            </label>
            <label>
              {t("customAgent.command")}
              <input
                value={manual.command}
                onChange={(e) => setManual((m) => ({ ...m, command: e.target.value }))}
                placeholder={t("customAgent.commandPlaceholder")}
              />
            </label>
            <label>
              {t("customAgent.argsLabel")}
              <input
                value={manual.args}
                onChange={(e) => setManual((m) => ({ ...m, args: e.target.value }))}
                placeholder="--acp"
              />
            </label>
            <label>
              {t("customAgent.npmPackageLabel")}
              <input
                value={manual.npmPackage}
                onChange={(e) => setManual((m) => ({ ...m, npmPackage: e.target.value }))}
              />
            </label>
            <button
              type="button"
              className="project-dialog__confirm"
              disabled={Boolean(busyId)}
              onClick={() => void addManual()}
            >
              {t("customAgent.add")}
            </button>
          </div>
        )}
        <div className="project-dialog__actions">
          <button type="button" className="project-dialog__cancel" onClick={onClose}>
            {t("common.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
