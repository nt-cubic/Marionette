import { Plus, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import {
  dshPluginAdd,
  dshPluginRemove,
  dshPluginsList,
  type DshPlugin,
} from "../lib/api";
import { BADGE_LABELS, badgeFor } from "../lib/pluginBadges";
import { useT } from "../lib/i18n";

/**
 * 通用插件面板。清单来自 DSH profile `acp`（`dsh plugin --profile acp`）；
 * 任何 agent 下都显示，便于安装/卸载。徽章标明当前 ACP 会话里是否真正生效。
 */
export function PluginsSection({
  activeAgentId,
  onCount,
}: {
  activeAgentId?: string | null;
  onCount?: (count: number) => void;
}) {
  const t = useT();
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
      setNote(`${t("plugins.installed", { name: value })}${summary ? `\n${summary}` : ""}`);
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
      setNote(t("plugins.removed", { name }));
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const list = Array.isArray(plugins) ? plugins : [];
  const listCount = list.length;
  useEffect(() => {
    onCount?.(listCount);
  }, [listCount, onCount]);

  return (
    <section className="context-card" id="context-plugins">
      <div className="context-card__heading">
        <span>{t("plugins.title")}</span>
        <span className="rail-shared__scope">DeepSeek Harness</span>
        <div className="context-card__heading-actions">
          <button
            className="pill-action pill-action--icon pill-action--sm"
            type="button"
            title={t("rail.pluginsRefresh")}
            aria-label={t("rail.pluginsRefresh")}
            disabled={busy || operating}
            onClick={() => void refresh()}
          >
            <RefreshCw size={11} />
          </button>
        </div>
      </div>

      {list.length === 0 ? (
        <p className="context-card__empty" style={{ marginTop: 8 }}>
          {t("rail.pluginsEmpty")}
        </p>
      ) : (
        <ul className="panel-entry-list">
          {list.map((plugin) => {
            const info = badgeFor(plugin.name);
            const effectiveHere = onDsh && info.badge !== "web";
            const scopeNote = onDsh
              ? effectiveHere
                ? t("rail.pluginActiveHere")
                : t("rail.pluginNotHere")
              : t("rail.pluginDshOnly");
            return (
              <li
                className="panel-entry"
                key={`${plugin.name}@${plugin.version}`}
                title={`${plugin.name} ${plugin.version}
${info.reason}
${scopeNote}`}
              >
                <div className="panel-entry__head">
                  <span className="panel-entry__title" title={plugin.name}>
                    {plugin.name}
                  </span>
                  <span className="panel-entry__version">{plugin.version}</span>
                  <span className={`panel-entry__badge is-${info.badge}`}>
                    {BADGE_LABELS[info.badge]}
                  </span>
                  <button
                    className="pill-action pill-action--icon pill-action--sm"
                    type="button"
                    title={t("rail.pluginRemove")}
                    aria-label={t("rail.pluginRemoveNamed", { name: plugin.name })}
                    disabled={operating}
                    onClick={() => void handleRemove(plugin.name)}
                  >
                    <Trash2 size={11} />
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <div className="panel-install">
        <Plus size={12} aria-hidden className="panel-install__icon" />
        <input
          className="panel-form__input"
          placeholder={t("rail.pluginInstallPlaceholder")}
          aria-label={t("rail.pluginInstallPlaceholder")}
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
          {operating ? "…" : t("rail.pluginInstall")}
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
