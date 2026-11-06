import { Plus, Power, RefreshCw, Trash2, Zap } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  universalPluginAdd,
  universalPluginProbe,
  universalPluginRemove,
  universalPluginsList,
  universalPluginSetEnabled,
} from "../lib/api";
import { useT } from "../lib/i18n";
import type { UniversalPlugin, UniversalPluginSpec } from "../lib/types";

/**
 * 共通插件面板（docs/plan-plugin-layer.md）。与 PluginsSection（DSH 专属）
 * 不同：这里登记的插件对所有 agent 生效——stdio 插件由 Marionette 托管
 * 子进程、经 localhost HTTP MCP 以 url 型条目注入 session/new（zcode 也
 * 吃），远程插件纯中继、真实凭据不进 agent 进程。
 *
 * 自取数组件（照 PluginsSection 的形态），App 不需要新 props。
 */

type Transport = "stdio" | "http";

function parseArgs(raw: string): string[] {
  return raw.trim().split(/\s+/).filter(Boolean);
}

/** KEY=VALUE 每行一条；空行与无 = 的行跳过。 */
function parseAssignLines(raw: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const eq = trimmed.indexOf("=");
    if (eq <= 0) continue;
    out[trimmed.slice(0, eq).trim()] = trimmed.slice(eq + 1).trim();
  }
  return out;
}

/** `Key: Value` 或 `Key=Value` 每行一条（HTTP 头）。 */
function parseHeaderLines(raw: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of raw.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const colon = trimmed.indexOf(":");
    if (colon > 0) {
      out[trimmed.slice(0, colon).trim()] = trimmed.slice(colon + 1).trim();
      continue;
    }
    const eq = trimmed.indexOf("=");
    if (eq > 0) out[trimmed.slice(0, eq).trim()] = trimmed.slice(eq + 1).trim();
  }
  return out;
}

export function UniversalPluginsSection({
  onCount,
  embedded = false,
}: {
  onCount?: (count: number) => void;
  /** Skip the outer card chrome — parent already titled this block 「插件」. */
  embedded?: boolean;
} = {}) {
  const t = useT();
  const [plugins, setPlugins] = useState<UniversalPlugin[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [operating, setOperating] = useState(false);

  // Install form
  const [formOpen, setFormOpen] = useState(false);
  const [specId, setSpecId] = useState("");
  const [transport, setTransport] = useState<Transport>("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [env, setEnv] = useState("");
  const [url, setUrl] = useState("");
  const [headers, setHeaders] = useState("");

  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      setPlugins(await universalPluginsList());
      setNote(null);
    } catch (error) {
      setPlugins([]);
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const resetForm = () => {
    setSpecId("");
    setTransport("stdio");
    setCommand("");
    setArgs("");
    setEnv("");
    setUrl("");
    setHeaders("");
  };

  const handleAdd = async () => {
    const id = specId.trim();
    if (!id || operating) return;
    setOperating(true);
    try {
      const spec: UniversalPluginSpec =
        transport === "stdio"
          ? {
              id,
              transport,
              command: command.trim(),
              args: parseArgs(args),
              env: parseAssignLines(env),
              url: "",
              headers: {},
            }
          : {
              id,
              transport,
              command: "",
              args: [],
              env: {},
              url: url.trim(),
              headers: parseHeaderLines(headers),
            };
      const installed = await universalPluginAdd(spec);
      const probe = installed.lastProbe
        ? installed.lastProbe.ok
          ? t("allPlugins.probeOk", { count: installed.lastProbe.toolCount ?? 0 })
          : `${t("allPlugins.probeFailed")} · ${installed.lastProbe.note}`
        : null;
      setNote(`${t("allPlugins.installed", { name: id })}${probe ? `\n${probe}` : ""}`);
      resetForm();
      setFormOpen(false);
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const handleRemove = async (id: string) => {
    setOperating(true);
    try {
      await universalPluginRemove(id);
      setNote(t("allPlugins.removed", { name: id }));
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const handleToggle = async (plugin: UniversalPlugin) => {
    setOperating(true);
    try {
      await universalPluginSetEnabled(plugin.id, !plugin.enabled);
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const handleProbe = async (id: string) => {
    setOperating(true);
    setNote(t("allPlugins.probing", { name: id }));
    try {
      const probe = await universalPluginProbe(id);
      setNote(
        probe.ok
          ? `${id}: ${t("allPlugins.probeOk", { count: probe.toolCount ?? 0 })}`
          : `${id}: ${t("allPlugins.probeFailed")} · ${probe.note}`,
      );
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const list = Array.isArray(plugins) ? plugins : [];
  useEffect(() => {
    if (plugins == null) return;
    onCount?.(plugins.length);
  }, [plugins, onCount]);

  const heading = (
    <div className={embedded ? "context-lend__labelrow" : "context-card__heading"}>
      {embedded ? (
        <span className="context-lend__label">{t("rail.pluginsAllAgents")}</span>
      ) : (
        <span>{t("allPlugins.title")}</span>
      )}
      <div className="context-card__heading-actions">
        <button
          className="pill-action pill-action--icon pill-action--sm"
          type="button"
          title={t("allPlugins.refresh")}
          aria-label={t("allPlugins.refresh")}
          disabled={busy || operating}
          onClick={() => void refresh()}
        >
          <RefreshCw size={11} />
        </button>
        <button
          className="pill-action pill-action--icon pill-action--sm"
          type="button"
          title={t("allPlugins.add")}
          aria-label={t("allPlugins.add")}
          disabled={operating}
          onClick={() => setFormOpen((open) => !open)}
        >
          <Plus size={12} />
        </button>
      </div>
    </div>
  );

  const inner = (
    <>
      {heading}

      {list.length === 0 && !formOpen && !embedded ? (
        <p className="context-card__empty" style={{ marginTop: 8 }}>
          {t("allPlugins.empty")}
        </p>
      ) : null}

      {list.length > 0 ? (
        <ul className="panel-entry-list">
          {list.map((plugin) => (
            <li className="panel-entry" key={plugin.id}>
              <div className="panel-entry__head">
                <span className="panel-entry__title" title={plugin.id}>
                  {plugin.id}
                </span>
                <span
                  className="panel-entry__version"
                  style={{ opacity: plugin.enabled ? 0.9 : 0.5 }}
                >
                  {plugin.transport}
                </span>
                <button
                  className="pill-action pill-action--icon pill-action--sm"
                  type="button"
                  title={plugin.enabled ? t("allPlugins.disable") : t("allPlugins.enable")}
                  aria-label={plugin.enabled ? t("allPlugins.disable") : t("allPlugins.enable")}
                  disabled={operating}
                  onClick={() => void handleToggle(plugin)}
                  style={{ opacity: plugin.enabled ? 1 : 0.5 }}
                >
                  <Power size={11} />
                </button>
                <button
                  className="pill-action pill-action--icon pill-action--sm"
                  type="button"
                  title={t("allPlugins.probe")}
                  aria-label={`${t("allPlugins.probe")} ${plugin.id}`}
                  disabled={operating}
                  onClick={() => void handleProbe(plugin.id)}
                >
                  <Zap size={11} />
                </button>
                <button
                  className="pill-action pill-action--icon pill-action--sm"
                  type="button"
                  title={t("allPlugins.remove")}
                  aria-label={`${t("allPlugins.remove")} ${plugin.id}`}
                  disabled={operating}
                  onClick={() => void handleRemove(plugin.id)}
                >
                  <Trash2 size={11} />
                </button>
              </div>
              <p className="panel-entry__text">
                {plugin.transport === "http" && plugin.url ? plugin.url : null}
                {plugin.transport === "stdio" && plugin.command
                  ? [plugin.command, ...(plugin.args ?? [])].join(" ")
                  : null}
                {plugin.enabled ? null : ` · ${t("allPlugins.disabledTag")}`}
                {plugin.lastProbe
                  ? plugin.lastProbe.ok
                    ? ` · ${t("allPlugins.probeOk", { count: plugin.lastProbe.toolCount ?? 0 })}`
                    : ` · ${t("allPlugins.probeFailed")} · ${plugin.lastProbe.note}`
                  : null}
              </p>
            </li>
          ))}
        </ul>
      ) : null}

      {formOpen ? (
        <div className="panel-install" style={{ flexDirection: "column", alignItems: "stretch", gap: 6 }}>
          <input
            className="panel-form__input"
            placeholder={t("allPlugins.idPlaceholder")}
            value={specId}
            disabled={operating}
            onChange={(e) => setSpecId(e.target.value)}
          />
          <select
            className="panel-form__input"
            value={transport}
            disabled={operating}
            onChange={(e) => setTransport(e.target.value === "http" ? "http" : "stdio")}
          >
            <option value="stdio">stdio</option>
            <option value="http">http</option>
          </select>
          {transport === "stdio" ? (
            <>
              <input
                className="panel-form__input"
                placeholder={t("allPlugins.commandPlaceholder")}
                value={command}
                disabled={operating}
                onChange={(e) => setCommand(e.target.value)}
              />
              <input
                className="panel-form__input"
                placeholder={t("allPlugins.argsPlaceholder")}
                value={args}
                disabled={operating}
                onChange={(e) => setArgs(e.target.value)}
              />
              <textarea
                className="panel-form__input"
                placeholder={t("allPlugins.envPlaceholder")}
                value={env}
                disabled={operating}
                rows={2}
                onChange={(e) => setEnv(e.target.value)}
              />
            </>
          ) : (
            <>
              <input
                className="panel-form__input"
                placeholder={t("allPlugins.urlPlaceholder")}
                value={url}
                disabled={operating}
                onChange={(e) => setUrl(e.target.value)}
              />
              <textarea
                className="panel-form__input"
                placeholder={t("allPlugins.headersPlaceholder")}
                value={headers}
                disabled={operating}
                rows={2}
                onChange={(e) => setHeaders(e.target.value)}
              />
            </>
          )}
          <button
            className="pill-action"
            type="button"
            disabled={!specId.trim() || operating || (transport === "stdio" ? !command.trim() : !url.trim())}
            onClick={() => void handleAdd()}
          >
            {operating ? "…" : t("allPlugins.install")}
          </button>
        </div>
      ) : null}

      {embedded ? null : (
        <p className="panel-note panel-note--hint" title={t("allPlugins.applyHint")}>
          {t("allPlugins.applyHint")}
        </p>
      )}

      {note ? (
        <p className="panel-note" style={{ whiteSpace: "pre-wrap" }}>
          {note}
        </p>
      ) : null}
    </>
  );

  if (embedded) return <div className="plugin-block">{inner}</div>;
  return (
    <section className="context-card" id="context-universal-plugins">
      {inner}
    </section>
  );
}
