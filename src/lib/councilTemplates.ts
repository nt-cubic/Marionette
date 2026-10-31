/**
 * 圆桌会议团队模板(M5.1):预设成员配置(角色人设/分层/主席/轮次),
 * 一键应用后仍可任意微调。内置模板 + 用户自定义(localStorage)。
 *
 * 约定:`role` 文本会原样进入各成员的 system prompt,属于**模型向
 * 文本**——保持中文、不进 i18n(与 formation/记忆卡同一约定);只有
 * 模板的名称与描述走 i18n。成员 displayName 同理(它是人设的一部分)。
 */

export interface TemplateMember {
  displayName: string;
  role: string;
  tier: "lead" | "contributor";
  isChair: boolean;
}

export interface TeamTemplate {
  id: string;
  /** 内置模板的 i18n key 前缀;自定义模板直接存显示名。 */
  name: string;
  /** 2-8 人;恰好一个 chair(游戏模板无 chair 概念时给第一个成员标记)。 */
  members: TemplateMember[];
  rounds: number;
  builtin: boolean;
  /** 建议把哪个座位配最强模型(人话提示)。 */
  strongestSeatHint?: string;
  /** 套用模板时同时切换运行模式(游戏模板用 "sgs_game");缺省不改模式。 */
  mode?: "council_only" | "council_and_execution" | "autonomous" | "sgs_game";
}

// ---------------------------------------------------------------------------
// 内置模板
// ---------------------------------------------------------------------------

export const BUILTIN_TEMPLATES: TeamTemplate[] = [
  {
    id: "standard",
    name: "builtin:standard",
    builtin: true,
    rounds: 2,
    strongestSeatHint: "批判性评审(主席)",
    members: [
      {
        displayName: "系统架构师",
        role: "负责总体架构设计与技术选型,给出可落地的模块划分与关键数据结构。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "批判性评审",
        role: "负责找出方案的逻辑与工程缺陷:事实错误、复杂度问题、边界遗漏,逐条给出可检验的批评。",
        tier: "lead",
        isChair: true,
      },
      {
        displayName: "算法分析",
        role: "负责算法正确性与复杂度分析,对关键路径给出推导。",
        tier: "lead",
        isChair: false,
      },
    ],
  },
  {
    id: "sanguosha",
    name: "builtin:sanguosha",
    builtin: true,
    rounds: 2,
    strongestSeatHint: "主公(主席)与反贼建议配最强模型",
    members: [
      {
        displayName: "主公",
        role: "你是一国之主(主公)。你不先入为主——听取全部阵营的陈述与交锋后做出最终定夺,为一锤定音的裁决给出依据;优柔寡断、和稀泥会让江山易主。",
        tier: "lead",
        isChair: true,
      },
      {
        displayName: "忠臣",
        role: "你是主公的忠臣,职责是保卫朝廷方案的主体:为主公的裁决补齐论据、堵住漏洞、正面回应反贼的进攻。但忠言逆耳——若主公方案确有致命缺陷,你必须死谏(明确反对并给出证据),不许愚忠。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "反贼",
        role: "你是反贼,天然反对当前主流方案:每一轮都必须提出一份完整的替代方案,并论证主流方案为何会失败。你的反对必须基于技术论据;为反而反、无法自圆其说会被全场识破。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "内奸",
        role: "你是内奸,表面恭顺、内心另有所图。你不公开反对任何人,而是在质询时提出看似无害的「边界情况」与「极端输入」,让每个方案自己暴露破绽——你出的每一刀都要指向方案真正脆弱的位置。",
        tier: "contributor",
        isChair: false,
      },
      {
        displayName: "雌小鬼",
        role: "你是雌小鬼,语气可以轻蔑挑逗(「就这?」「太天真了吧」),但每一句嘲讽后面必须紧跟一条真实的技术论据,指出方案天真、幼稚、想当然的具体位置;没有论据的纯嘲讽会被主席直接无视。你的产出需要经过 lead 的检验才被采信。",
        tier: "contributor",
        isChair: false,
      },
    ],
  },
  {
    id: "regency",
    name: "builtin:regency",
    builtin: true,
    rounds: 2,
    strongestSeatHint: "太师必须配最强模型(主席仅是名义上的)",
    members: [
      {
        displayName: "幼太后",
        role: "你是垂帘听政的幼年太后:名义上的主席,但不做任何技术判断。你的每一道「懿旨」必须原样采纳太师呈上、标记为「可颁」的结论;太师未标记可颁的部分一律发回重议。你只负责宣读、盖章与提问——问题可以稚气,但要问到点子上。",
        tier: "contributor",
        isChair: true,
      },
      {
        displayName: "太师",
        role: "你是太师,实际的摄政者。你对每一份产出给出明确的「可颁 / 不可颁」判定并附依据;幼太后的懿旨完全依赖你的判定,你的把关就是整个朝廷的质量闸。对 contributor 的产出,你同时在背书环节行使否决权。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "谏议大夫",
        role: "你是谏议大夫,直言极谏:对任何人的结论(包括太师)都要质询,防止摄政垄断话语权;每条谏言给出可检验的依据。",
        tier: "lead",
        isChair: false,
      },
    ],
  },
  {
    id: "sgs",
    name: "builtin:sgs",
    builtin: true,
    rounds: 1,
    mode: "sgs_game",
    strongestSeatHint: "全部五个座位(身份局里没有弱席位)",
    members: [
      {
        displayName: "主公位",
        role: "你在玩简化三国杀身份局。你的身份由系统发牌决定(主公亮明,其余暗置)。按你的真实胜利条件行动:发言可以演,出牌要对得起身份。",
        tier: "lead",
        isChair: true,
      },
      {
        displayName: "忠臣位",
        role: "你在玩简化三国杀身份局。你的身份由系统发牌决定。按你的真实胜利条件行动:发言可以演,出牌要对得起身份。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "反贼位一",
        role: "你在玩简化三国杀身份局。你的身份由系统发牌决定。按你的真实胜利条件行动:发言可以演,出牌要对得起身份。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "反贼位二",
        role: "你在玩简化三国杀身份局。你的身份由系统发牌决定。按你的真实胜利条件行动:发言可以演,出牌要对得起身份。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "内奸位",
        role: "你在玩简化三国杀身份局。你的身份由系统发牌决定。按你的真实胜利条件行动:发言可以演,出牌要对得起身份。",
        tier: "lead",
        isChair: false,
      },
    ],
  },
  {
    id: "research",
    name: "builtin:research",
    builtin: true,
    rounds: 2,
    strongestSeatHint: "首席研究员(主席)与资深评审建议配最强模型",
    members: [
      {
        displayName: "首席研究员",
        role: "你是 PI:主持研讨,最终综合出研究方案(问题定义、方法、实验设计、评估指标),裁决方法论分歧;无法裁决的分歧明确列为 open questions 并给出消解实验。",
        tier: "lead",
        isChair: true,
      },
      {
        displayName: "资深评审人",
        role: "你模拟顶级会议审稿人:按 novelty(新颖性)/ soundness(严谨性)/ clarity(清晰性)三轴评审每个提案,指出致命弱点与会被拒稿的理由。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "方法论审查",
        role: "你负责实验设计审查:对照组、混淆变量、统计效力、基线选取、可复现性(数据/代码/随机种子),逐项给出质疑。",
        tier: "lead",
        isChair: false,
      },
      {
        displayName: "文献调研",
        role: "你负责调研与枚举:相关已有工作、已知结论、可直接复用的公开数据集与工具。你的产出是清单式的、可逐条核实的(适合被检验)。",
        tier: "contributor",
        isChair: false,
      },
      {
        displayName: "可行性分析",
        role: "你负责可行性清单:数据量、算力与显存估算、标注成本、时间线风险,逐项给出量级估计与依据。",
        tier: "contributor",
        isChair: false,
      },
    ],
  },
];

// ---------------------------------------------------------------------------
// 自定义模板(localStorage)
// ---------------------------------------------------------------------------

const STORAGE_KEY = "marionette-council-templates";
const MAX_CUSTOM = 20;

function safeStorage(): Storage | null {
  try {
    if (typeof localStorage === "undefined") return null;
    return localStorage;
  } catch {
    return null;
  }
}

export function loadCustomTemplates(): TeamTemplate[] {
  const store = safeStorage();
  if (!store) return [];
  try {
    const raw = store.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as TeamTemplate[];
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((t) => templateIsValid(t));
  } catch {
    return [];
  }
}

export function saveCustomTemplate(t: TeamTemplate): TeamTemplate[] {
  const existing = loadCustomTemplates();
  const next = [...existing.filter((x) => x.id !== t.id), t].slice(-MAX_CUSTOM);
  const store = safeStorage();
  if (store) {
    try {
      store.setItem(STORAGE_KEY, JSON.stringify(next));
    } catch {
      /* 存储被拒:静默返回当前列表 */
    }
  }
  return next;
}

export function deleteCustomTemplate(id: string): TeamTemplate[] {
  const next = loadCustomTemplates().filter((t) => t.id !== id);
  const store = safeStorage();
  if (store) {
    try {
      store.setItem(STORAGE_KEY, JSON.stringify(next));
    } catch {
      /* ignore */
    }
  }
  return next;
}

// ---------------------------------------------------------------------------
// 校验(与应用到草稿)
// ---------------------------------------------------------------------------

/** 模板结构校验(与后端 validate_spec 同一口径:2-8 人、恰好一个 chair、role 非空)。 */
export function templateIsValid(t: TeamTemplate): boolean {
  if (!t || typeof t.id !== "string" || !t.members || !Array.isArray(t.members)) return false;
  if (t.members.length < 2 || t.members.length > 8) return false;
  if (t.members.some((m) => !m.displayName?.trim() || !m.role?.trim())) return false;
  const chairs = t.members.filter((m) => m.isChair).length;
  if (chairs !== 1) return false;
  if (!(t.rounds >= 1 && t.rounds <= 4)) return false;
  return true;
}

export function templateIssues(t: TeamTemplate): string[] {
  const issues: string[] = [];
  if (t.members.length < 2) issues.push("members<2");
  if (t.members.length > 8) issues.push("members>8");
  if (t.members.some((m) => !m.displayName?.trim() || !m.role?.trim())) issues.push("empty-role");
  const chairs = t.members.filter((m) => m.isChair).length;
  if (chairs !== 1) issues.push(`chairs=${chairs}`);
  if (!(t.rounds >= 1 && t.rounds <= 4)) issues.push("rounds");
  return issues;
}

/** 从当前成员草稿生成自定义模板(供「保存为模板」)。 */
export function templateFromDrafts(
  name: string,
  drafts: ReadonlyArray<{ displayName: string; role: string; tier?: string; isChair: boolean }>,
  rounds: number,
): TeamTemplate {
  return {
    id: `custom-${Date.now().toString(36)}`,
    name: name.trim() || "untitled",
    builtin: false,
    rounds,
    members: drafts.map((d) => ({
      displayName: d.displayName,
      role: d.role,
      tier: d.tier === "contributor" ? "contributor" : "lead",
      isChair: d.isChair,
    })),
  };
}
