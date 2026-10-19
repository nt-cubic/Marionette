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
 * DSH 插件面板（极简版）：清单 + 徽章 + 安装/卸载。
 * 安装动作本身就是 `dsh plugin --profile acp …`；用户也可以直接在聊天里
 * 让 AI 装（AI 跑命令走正常权限确认）。装/卸后需重启会话生效。
 */
export function PluginsSection() {
  const [plugins, setPlugins] = useState<DshPlugin[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [spec, setSpec] = useState("");
  const [operating, setOperating] = useState(false);

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
      setNote(`已安装 ${value}（重启会话生效）${summary ? `\n${summary}` : ""}`);
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
      setNote(`已卸载 ${name}（重启会话后彻底生效）`);
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
        <span>DSH 插件 · Plugins</span>
        <div className="context-card__heading-actions">
          <button
            className="pill-action pill-action--icon pill-action--sm"
            type="button"
            title="刷新插件清单"
            aria-label="Refresh dsh plugins"
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
          还没有插件。填包名安装，或直接在聊天里让 AI 装。
        </p>
      ) : (
        <ul className="panel-entry-list">
          {list.map((plugin) => {
            const info = badgeFor(plugin.name);
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
                    title="卸载这个插件"
                    aria-label={`Remove ${plugin.name}`}
                    disabled={operating}
                    onClick={() => void handleRemove(plugin.name)}
                  >
                    <Trash2 size={11} />
                  </button>
                </div>
                <p className="panel-entry__text" title={info.reason}>
                  {info.reason}
                </p>
              </li>
            );
          })}
        </ul>
      )}

      <div className="panel-install">
        <input
          className="panel-form__input"
          placeholder="包名（如 dsh-pet）/ github:作者/仓库"
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
          {operating ? "执行中…" : "安装"}
        </button>
      </div>

      <p className="panel-note">装/卸后需重启会话生效；插件是第三方代码，只装信任来源。</p>

      {note ? (
        <p className="panel-note" style={{ whiteSpace: "pre-wrap" }}>
          {note}
        </p>
      ) : null}
    </section>
  );
}
