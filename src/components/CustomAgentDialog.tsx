import { useCallback, useEffect, useMemo, useState } from "react";
import {
  addCustomAgent,
  fetchAcpRegistry,
  type AcpCatalogAgent,
  type CustomAgentDef,
} from "../lib/api";
import type { AgentConfig } from "../lib/types";

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
        `${entry.name} 只有 binary 分发，Marionette 不下载安装包。把可执行文件放到 PATH 后用「手动添加」。`,
      );
      return;
    }
    let id = safeId(entry.id);
    if (taken(id)) {
      if (builtinIds.has(id)) {
        setFormError(`${entry.name} 已经是内置 agent（${id}），不用再加。`);
        return;
      }
      id = safeId(`custom-${entry.id}`);
    }
    if (taken(id)) {
      setFormError(`已经添加过 ${entry.name}。`);
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
      onAdded(`已添加「${entry.name}」。`);
    } catch (e) {
      setFormError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusyId(null);
    }
  };

  const addManual = async () => {
    const id = safeId(manual.id);
    if (!id || (taken(id) && builtinIds.has(id))) {
      setFormError(builtinIds.has(id) ? `id \`${id}\` 已被内置占用` : "id 无效");
      return;
    }
    if (!manual.label.trim() || !manual.command.trim()) {
      setFormError("显示名称和命令都要填。");
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
      onAdded(`已添加「${manual.label.trim()}」。`);
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
        <h2 id="custom-agent-title">添加自定义 agent</h2>
        <p className="custom-agent-dialog__lead">
          从公开 ACP 注册表挑一个，或手填命令。npx / uvx 可一键装；binary 请自行放到 PATH。
        </p>
        <div className="custom-agent-dialog__tabs">
          <button
            type="button"
            className={tab === "registry" ? "is-selected" : ""}
            onClick={() => setTab("registry")}
          >
            ACP 注册表
          </button>
          <button
            type="button"
            className={tab === "manual" ? "is-selected" : ""}
            onClick={() => setTab("manual")}
          >
            手动添加
          </button>
        </div>
        {tab === "registry" ? (
          <>
            <input
              className="custom-agent-dialog__search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="搜索名称 / id"
              autoFocus
            />
            {loadError && (
              <p className="custom-agent-dialog__error">
                {loadError}{" "}
                <button type="button" onClick={() => void load()}>
                  重试
                </button>
              </p>
            )}
            {formError && <p className="custom-agent-dialog__error">{formError}</p>}
            {!catalog && !loadError && <p className="custom-agent-dialog__hint">正在拉取注册表…</p>}
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
                          ? "已内置"
                          : already
                            ? "已添加"
                            : entry.installable
                              ? "添加"
                              : "无法安装"}
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
              显示名称
              <input
                value={manual.label}
                onChange={(e) => setManual((m) => ({ ...m, label: e.target.value }))}
              />
            </label>
            <label>
              命令
              <input
                value={manual.command}
                onChange={(e) => setManual((m) => ({ ...m, command: e.target.value }))}
                placeholder="PATH 上的可执行文件"
              />
            </label>
            <label>
              参数（空格分隔）
              <input
                value={manual.args}
                onChange={(e) => setManual((m) => ({ ...m, args: e.target.value }))}
                placeholder="--acp"
              />
            </label>
            <label>
              npm 包名（可选，有则支持 Install）
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
              添加
            </button>
          </div>
        )}
        <div className="project-dialog__actions">
          <button type="button" className="project-dialog__cancel" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
