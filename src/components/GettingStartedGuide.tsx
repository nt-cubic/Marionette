import { useMemo, useState } from "react";
import { ChevronRight, X } from "lucide-react";
import { SUBSCRIPTION_FAMILIES, useModelLibrary, type SubscriptionFamily } from "../lib/modelLibrary";
import { t as translate, useT } from "../lib/i18n";
import type { AgentConfig } from "../lib/types";
import { ModelSettingsDialog } from "./ModelSettingsDialog";

/**
 * 首次启动引导（2026-11-27）：一页式「开始使用」，不做多步向导。
 *
 * 目标不是讲清所有可能，而是让用户 30 秒内发出第一句话：
 * 1. 自动检测本机已有条件（已登录订阅四族、已存 API 来源、已启用 agent）；
 * 2. 检测到任意一条 → 主按钮就是「开始第一场对话」（会话用当前 agent 的默认模型，
 *    不强行先配来源）；旁边进「模型设置」细调；
 * 3. 一条都没有 → 三条路：登录订阅（各 CLI 怎么登）/ 添加 API Key 或自定义来源 /
 *    先用本机 agent 默认模型开一场对话。
 *
 * 自动出现的条件：还没有任何会话、且没被跳过过（localStorage 一枚标记）。随时可从
 * Ctrl K「开始使用」重开。
 */
const DONE_KEY = "marionette-getting-started-v1";

const FAMILY_LABEL_KEY = {
  claude: "models.family_claude",
  codex: "models.family_codex",
  grok: "models.family_grok",
  gemini: "models.family_gemini",
} as const;

function readDone(): boolean {
  try { return localStorage.getItem(DONE_KEY) === "1"; } catch { return false; }
}

export function GettingStartedGuide({ agents, hasSessions, open, onClose, onCreateChat }: {
  agents: AgentConfig[];
  /** 已经有会话（本机不是「什么都没有」）→ 不自动弹。 */
  hasSessions: boolean;
  /** Ctrl K「开始使用」/ 其它入口手动打开（无视跳过标记）。 */
  open: boolean;
  onClose: () => void;
  onCreateChat: () => void;
}) {
  const i18n = useT();
  const { sources, signedIn } = useModelLibrary(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [dismissed, setDismissed] = useState(readDone);
  const visible = open || (!hasSessions && !dismissed);
  const logged = useMemo(() => new Set(signedIn), [signedIn]);
  const sourceCount = sources.filter(source => source.models.length > 0).length;
  const enabledAgents = agents.filter(agent => agent.enabled).length;
  const available = signedIn.length > 0 || sourceCount > 0;

  if (!visible) return null;
  const close = () => {
    try { localStorage.setItem(DONE_KEY, "1"); } catch { /* private mode */ }
    setDismissed(true);
    onClose();
  };
  const startChat = () => { close(); onCreateChat(); };
  return <div className="project-dialog-backdrop" onClick={close}>
    <div className="project-dialog getting-started" role="dialog" aria-modal="true" aria-label={translate("guide.title")} onClick={event => event.stopPropagation()}>
      <div className="getting-started__head">
        <span className="getting-started__title">{i18n("guide.title")}</span>
        <button type="button" className="getting-started__close" aria-label={i18n("common.close")} onClick={close}><X size={14} /></button>
      </div>
      <p className="getting-started__lead">{i18n("guide.lead")}</p>
      <div className="getting-started__detect">
        <div><span>{i18n("guide.detectSubs")}</span><b>{signedIn.length > 0 ? i18n("guide.detectedSubs", { count: signedIn.length }) : i18n("guide.noneSubs")}</b></div>
        <div><span>{i18n("guide.detectSources")}</span><b>{sourceCount > 0 ? i18n("guide.detectedSources", { count: sourceCount }) : i18n("guide.noneSources")}</b></div>
        <div><span>{i18n("guide.detectAgents")}</span><b>{i18n("guide.detectedAgents", { count: enabledAgents })}</b></div>
      </div>
      {available ? <div className="getting-started__ready">
        <p>{i18n("guide.ready")}</p>
        <div className="getting-started__actions">
          <button type="button" className="project-dialog__submit" onClick={startChat}>{i18n("guide.startChat")}</button>
          <button type="button" className="project-dialog__cancel" onClick={() => setSettingsOpen(true)}>{i18n("models.settingsTitle")}</button>
        </div>
      </div> : <div className="getting-started__paths">
        <div className="getting-started__path">
          <b>{i18n("guide.pathSubs")}</b>
          <span>{i18n("guide.pathSubsHint")}</span>
          <div className="getting-started__families">
            {SUBSCRIPTION_FAMILIES.map((family: SubscriptionFamily) => <span key={family} className={logged.has(family) ? "is-on" : ""}>
              {i18n(FAMILY_LABEL_KEY[family])} · {i18n(logged.has(family) ? "guide.signedIn" : "guide.notSignedIn")}
            </span>)}
          </div>
          <button type="button" className="getting-started__more" onClick={() => setSettingsOpen(true)}>{i18n("guide.openSettings")} <ChevronRight size={12} aria-hidden /></button>
        </div>
        <div className="getting-started__path">
          <b>{i18n("guide.pathKey")}</b>
          <span>{i18n("guide.pathKeyHint")}</span>
          <button type="button" className="getting-started__more" onClick={() => setSettingsOpen(true)}>{i18n("guide.addSource")} <ChevronRight size={12} aria-hidden /></button>
        </div>
        <div className="getting-started__path">
          <b>{i18n("guide.pathTry")}</b>
          <span>{i18n("guide.pathTryHint")}</span>
          <button type="button" className="getting-started__more" onClick={startChat}>{i18n("guide.startChat")} <ChevronRight size={12} aria-hidden /></button>
        </div>
      </div>}
      <div className="getting-started__footer">
        <span>{i18n("guide.reopen")}</span>
        <button type="button" className="getting-started__skip" onClick={close}>{i18n("guide.skip")}</button>
      </div>
      {settingsOpen && <ModelSettingsDialog agents={agents} onClose={() => setSettingsOpen(false)} />}
    </div>
  </div>;
}
