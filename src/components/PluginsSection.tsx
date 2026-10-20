import { Puzzle, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  dshPluginAdd,
  dshPluginRemove,
  dshPluginsList,
  type DshPlugin,
} from "../lib/api";
import { BADGE_COLORS, BADGE_LABELS, badgeFor } from "../lib/pluginBadges";

/**
 * 通用插件面板。清单来自 DSH profile `acp`（`dsh plugin --profile acp`）；
 * 任何 agent 下都显示，便于安装/卸载。徽章标明当前 ACP 会话里是否真正生效。
 */
export function PluginsSection({ activeAgentId }: { activeAgentId?: string | null }) {
  const [plugins, setPlugins] = useState<DshPlugin[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [spec, setSpec] = useState("");
  const [operating, setOperating] = useState(false);

  const onDsh = (activeAgentId ?? "").startsWith("deepseek");

  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      setPlugins(await dshPluginsList());
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

  const handleAdd = async () => {
    const value = spec.trim();
    if (!value) return;
    setOperating(true);
    try {
      const { summary } = await dshPluginAdd(value);
      setNote(`已安装 ${value}（重启 DSH 会话生效）${summary ? `\n${summary}` : ""}`);
      setSpec("");
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const handleRemove = async (name: string) => {
    setOperating(true);
    try {
      await dshPluginRemove(name);
      setNote(`已卸载 ${name}（重启 DSH 会话后彻底生效）`);
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const list = Array.isArray(plugins) ? plugins : [];

  return (
    <section className="context-card">
      <div className="context-card__heading">
        <span>PLUGINS</span>
        <div className="context-card__heading-actions">
          <button
            className="pill-action pill-action--icon pill-action--sm"
            type="button"
            title="Refresh plugins"
            aria-label="Refresh plugins"
            disabled={busy || operating}
            onClick={() => void refresh()}
          >
            <RefreshCw size={11} />
          </button>
        </div>
      </div>

      {list.length === 0 ? (
        <p className="context-card__empty" style={{ marginTop: 8 }}>
          <Puzzle size={11} style={{ verticalAlign: -1, marginRight: 4 }} />
          No plugins installed. Paste a package name below.
        </p>
      ) : (
        <ul className="panel-entry-list">
          {list.map((plugin) => {
            const info = badgeFor(plugin.name);
            const effectiveHere = onDsh && info.badge !== "web";
            return (
              <li
                className="panel-entry"
                key={`${plugin.name}@${plugin.version}`}
              >
                <div className="panel-entry__head">
                  <span className="panel-entry__title" title={plugin.name}>
                    {plugin.name}
                  </span>
                  <span className="panel-entry__version">{plugin.version}</span>
                  <span
                    className="panel-entry__badge"
                    style={{ background: BADGE_COLORS[info.badge] }}
                    title={info.reason}
                  >
                    {BADGE_LABELS[info.badge]}
                  </span>
                  <button
                    className="pill-action pill-action--icon pill-action--sm"
                    type="button"
                    title="Uninstall"
                    aria-label={`Remove ${plugin.name}`}
                    disabled={operating}
                    onClick={() => void handleRemove(plugin.name)}
                  >
                    <Trash2 size={11} />
                  </button>
                </div>
                <p className="panel-entry__text" title={info.reason}>
                  {info.reason}
                  {onDsh
                    ? effectiveHere
                      ? " · active in this ACP"
                      : " · not effective in this ACP"
                    : " · applies to DeepSeek Harness sessions"}
                </p>
              </li>
            );
          })}
        </ul>
      )}

      <div className="panel-install">
        <input
          className="panel-form__input"
          placeholder="package (e.g. dsh-pet) / github:owner/repo"
          value={spec}
          disabled={operating}
          onChange={(e) => setSpec(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && spec.trim() && !operating) void handleAdd();
          }}
        />
        <button
          className="pill-action"
          type="button"
          disabled={!spec.trim() || operating}
          onClick={() => void handleAdd()}
        >
          {operating ? "…" : "Install"}
        </button>
      </div>

      {note ? (
        <p className="panel-note" style={{ whiteSpace: "pre-wrap" }}>
          {note}
        </p>
      ) : null}
    </section>
  );
}
