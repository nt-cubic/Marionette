import { FolderPlus, Power, RefreshCw, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { pickFolder, skillInstall, skillRemove, skillSetEnabled, universalSkillsList } from "../lib/api";
import { useT } from "../lib/i18n";
import type { UniversalSkill } from "../lib/types";

/**
 * 插件卡的「技能」子块（2026-11-06 并入插件层）。技能 = ~/.marionette/
 * plugins/skills/ 下含 SKILL.md 的文件夹：装进来 → context_inventory 按
 * 同一口径扫到、「项目上下文→技能」与注入同步出现；停用（.disabled 标记）
 * /删除 → 同步消失。两个入口看同一份真相。与 UniversalPluginsSection
 * 同形态（自取数，App 不需要新 props）。
 */

export function UniversalSkillsSection({
  onCount,
  embedded = false,
}: {
  onCount?: (count: number) => void;
  /** Skip the outer card chrome — parent already titled this block 「插件」. */
  embedded?: boolean;
} = {}) {
  const t = useT();
  const [skills, setSkills] = useState<UniversalSkill[] | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [operating, setOperating] = useState(false);

  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      setSkills(await universalSkillsList());
      setNote(null);
    } catch (error) {
      setSkills([]);
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleInstall = async () => {
    if (operating) return;
    setOperating(true);
    try {
      const sourcePath = await pickFolder();
      if (!sourcePath) return;
      setNote(t("allPlugins.skillInstalling"));
      const installed = await skillInstall(sourcePath);
      setNote(t("allPlugins.skillInstalled", { name: installed.name || installed.id }));
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const handleToggle = async (skill: UniversalSkill) => {
    setOperating(true);
    try {
      await skillSetEnabled(skill.id, !skill.enabled);
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const handleRemove = async (skill: UniversalSkill) => {
    setOperating(true);
    try {
      await skillRemove(skill.id);
      setNote(t("allPlugins.skillRemoved", { name: skill.name || skill.id }));
      await refresh();
    } catch (error) {
      setNote(error instanceof Error ? error.message : String(error));
    } finally {
      setOperating(false);
    }
  };

  const list = Array.isArray(skills) ? skills : [];
  useEffect(() => {
    if (skills == null) return;
    onCount?.(skills.length);
  }, [skills, onCount]);

  const heading = (
    <div className={embedded ? "context-lend__labelrow" : "context-card__heading"}>
      <span className={embedded ? "context-lend__label" : undefined}>{t("allPlugins.skillTitle")}</span>
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
          title={t("allPlugins.skillInstall")}
          aria-label={t("allPlugins.skillInstall")}
          disabled={operating}
          onClick={() => void handleInstall()}
        >
          <FolderPlus size={12} />
        </button>
      </div>
    </div>
  );

  const inner = (
    <>
      {heading}

      {list.length === 0 && !embedded ? (
        <p className="context-card__empty" style={{ marginTop: 8 }}>
          {t("allPlugins.skillEmpty")}
        </p>
      ) : null}

      {list.length > 0 ? (
        <ul className="panel-entry-list">
          {list.map((skill) => (
            <li className="panel-entry" key={skill.id}>
              <div className="panel-entry__head">
                <span className="panel-entry__title" title={`${skill.name} (${skill.dir})`}>
                  {skill.name || skill.id}
                </span>
                <span className="panel-entry__version" style={{ opacity: skill.enabled ? 0.9 : 0.5 }}>
                  {skill.id}
                </span>
                <button
                  className="pill-action pill-action--icon pill-action--sm"
                  type="button"
                  title={skill.enabled ? t("allPlugins.skillDisable") : t("allPlugins.skillEnable")}
                  aria-label={
                    skill.enabled ? t("allPlugins.skillDisable") : t("allPlugins.skillEnable")
                  }
                  disabled={operating}
                  onClick={() => void handleToggle(skill)}
                  style={{ opacity: skill.enabled ? 1 : 0.5 }}
                >
                  <Power size={11} />
                </button>
                <button
                  className="pill-action pill-action--icon pill-action--sm"
                  type="button"
                  title={t("allPlugins.skillRemoveNamed", { name: skill.id })}
                  aria-label={t("allPlugins.skillRemoveNamed", { name: skill.id })}
                  disabled={operating}
                  onClick={() => void handleRemove(skill)}
                >
                  <Trash2 size={11} />
                </button>
              </div>
              {skill.description ? <p className="panel-entry__text">{skill.description}</p> : null}
              {skill.enabled ? null : (
                <p className="panel-entry__text">{` · ${t("allPlugins.skillDisabledTag")}`}</p>
              )}
            </li>
          ))}
        </ul>
      ) : null}

      {note ? (
        <p className="panel-note" style={{ whiteSpace: "pre-wrap" }}>
          {note}
        </p>
      ) : null}
    </>
  );

  if (embedded) return <div className="plugin-block">{inner}</div>;
  return <section className="context-card">{inner}</section>;
}
