import { t, type DictKey } from "./i18n";

/**
 * DSH 插件兼容徽章（Steam Deck 式三档 + 未知）。
 *
 * 数据来自 2026-10-19 对 awesome-dsh-plugin.com 热门榜的人工分类（AGENTS.md
 * 对应轮调研）。分档标准：
 * - full：能力型（tools/commands/skills/subagent），装完经 ACP 自动生效
 * - partial：后台真干活，但自带仪表盘/管理页是 DSH 网页版（木偶里看不到）
 * - web：纯改造 DSH 网页界面，在木偶中无效果
 * 未收录的走启发式（名字像界面类 → web），再不行 → unknown（让用户问 AI）。
 */

export type PluginBadge = "full" | "partial" | "web" | "unknown";

export type PluginBadgeInfo = {
  badge: PluginBadge;
  reason: string;
};

const BADGES: Record<string, { badge: PluginBadge; reasonKey: DictKey }> = {
  // 🟢 能力型：装完直接生效
  "dsh-pet": { badge: "full", reasonKey: "badge.dshPet" },
  "billion-context": { badge: "full", reasonKey: "badge.billionContext" },
  "dsh-agency-agents": { badge: "full", reasonKey: "badge.dshAgencyAgents" },
  "dsh-im-connect": { badge: "full", reasonKey: "badge.dshImConnect" },
  "dsh-automation": { badge: "full", reasonKey: "badge.dshAutomation" },
  modlens: { badge: "full", reasonKey: "badge.modlens" },
  "dsh-btw": { badge: "full", reasonKey: "badge.dshBtw" },
  "dsh-simplify": { badge: "full", reasonKey: "badge.dshSimplify" },
  "dsh-code-review": { badge: "full", reasonKey: "badge.dshCodeReview" },
  // 🟡 后台生效、自带面板看不到
  "dsh-context": { badge: "partial", reasonKey: "badge.dshContext" },
  "dsh-task-board": { badge: "partial", reasonKey: "badge.dshTaskBoard" },
  "dsh-cost-meter": { badge: "partial", reasonKey: "badge.dshCostMeter" },
  "dsh-skills-manager": { badge: "partial", reasonKey: "badge.dshSkillsManager" },
  "dsh-archive-manager": { badge: "partial", reasonKey: "badge.dshArchiveManager" },
  "dsh-ssh": { badge: "partial", reasonKey: "badge.dshSsh" },
  // 🔴 网页版专用
  "dsh-codex-ui": { badge: "web", reasonKey: "badge.dshCodexUi" },
  "dsh-better-sidebar": { badge: "web", reasonKey: "badge.dshBetterSidebar" },
  "deepseek-balance-whale-widget": { badge: "web", reasonKey: "badge.balanceWhaleWidget" },
  "dsh-git-graph": { badge: "web", reasonKey: "badge.dshGitGraph" },
  "dsh-remote-web-ui": { badge: "web", reasonKey: "badge.dshRemoteWebUi" },
  "dsh-web-all": { badge: "web", reasonKey: "badge.dshWebAll" },
  "dsh-codex-pet": { badge: "web", reasonKey: "badge.dshCodexPet" },
  "dsh-market": { badge: "web", reasonKey: "badge.dshMarket" },
};

export function badgeFor(name: string): PluginBadgeInfo {
  const direct = BADGES[name.toLowerCase()] ?? BADGES[name];
  if (direct) return { badge: direct.badge, reason: t(direct.reasonKey) };
  const lower = name.toLowerCase();
  if (/(theme|skin|sidebar|widget|web-ui|-ui$)/.test(lower)) {
    return { badge: "web", reason: t("badge.heuristicWeb") };
  }
  return { badge: "unknown", reason: t("badge.uncatalogued") };
}

export const BADGE_LABELS: Record<PluginBadge, string> = {
  get full() {
    return t("badge.full");
  },
  get partial() {
    return t("badge.partial");
  },
  get web() {
    return t("badge.web");
  },
  get unknown() {
    return t("badge.unknown");
  },
};

export const BADGE_COLORS: Record<PluginBadge, string> = {
  full: "#3fb950",
  partial: "#d29922",
  web: "#8b949e",
  unknown: "#6e7681",
};
