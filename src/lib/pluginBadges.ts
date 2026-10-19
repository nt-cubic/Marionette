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

const BADGES: Record<string, PluginBadgeInfo> = {
  // 🟢 能力型：装完直接生效
  "dsh-pet": { badge: "full", reason: "桌宠住桌面（自带小窗），联动会话状态" },
  "billion-context": { badge: "full", reason: "上下文压缩，token 省约 5 倍" },
  "dsh-agency-agents": { badge: "full", reason: "领域专家子代理团" },
  "dsh-im-connect": { badge: "full", reason: "微信/飞书/钉钉/Telegram 遥控会话" },
  "dsh-automation": { badge: "full", reason: "定时在隔离会话跑任务" },
  modlens: { badge: "full", reason: "文字模型的视觉桥（贴图→结构化 JSON）" },
  "dsh-btw": { badge: "full", reason: "/btw 旁路提问，不进主对话" },
  "dsh-simplify": { badge: "full", reason: "/simplify 按 Git 范围简化代码" },
  "dsh-code-review": { badge: "full", reason: "/review 子代理代码审查" },
  // 🟡 后台生效、自带面板看不到
  "dsh-context": { badge: "partial", reason: "/context 可用；图表页在 DSH 网页版" },
  "dsh-task-board": { badge: "partial", reason: "定时任务后台真跑；看板在网页版" },
  "dsh-cost-meter": { badge: "partial", reason: "成本数据有；仪表盘在网页版" },
  "dsh-skills-manager": { badge: "partial", reason: "技能经 ACP 生效；管理页在网页版" },
  "dsh-archive-manager": { badge: "partial", reason: "归档在后台；页面在网页版" },
  "dsh-ssh": { badge: "partial", reason: "能力部分生效；终端面板在网页版" },
  // 🔴 网页版专用
  "dsh-codex-ui": { badge: "web", reason: "重构 DSH 网页侧边栏" },
  "dsh-better-sidebar": { badge: "web", reason: "网页版侧边栏工作台" },
  "deepseek-balance-whale-widget": { badge: "web", reason: "网页版角落挂件" },
  "dsh-git-graph": { badge: "web", reason: "网页版 Git 分支图" },
  "dsh-remote-web-ui": { badge: "web", reason: "手机遥控用的网页 UI" },
  "dsh-web-all": { badge: "web", reason: "网页版全家桶（看板/皮肤/宠物）" },
  "dsh-codex-pet": { badge: "web", reason: "网页内嵌宠物（不是桌宠）" },
  "dsh-market": { badge: "web", reason: "网页版插件商店面板" },
};

export function badgeFor(name: string): PluginBadgeInfo {
  const direct = BADGES[name.toLowerCase()] ?? BADGES[name];
  if (direct) return direct;
  const lower = name.toLowerCase();
  if (/(theme|skin|sidebar|widget|web-ui|-ui$)/.test(lower)) {
    return { badge: "web", reason: "按名字看是网页版界面类插件" };
  }
  return { badge: "unknown", reason: "未收录——装之前可以在聊天里让 AI 查一下说明" };
}

export const BADGE_LABELS: Record<PluginBadge, string> = {
  full: "完全支持",
  partial: "部分功能受限",
  web: "网页版专用",
  unknown: "未知",
};

export const BADGE_COLORS: Record<PluginBadge, string> = {
  full: "#3fb950",
  partial: "#d29922",
  web: "#8b949e",
  unknown: "#6e7681",
};
