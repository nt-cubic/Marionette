import { t } from "./i18n";
import type { SessionEvent, SessionStatus } from "./types";

export type SuggestionSourceId =
  | "drop" // 刚拖进来文件
  | "options" // AI 给了编号选项
  | "question" // AI 在问你要不要 X
  | "error" // 上一轮失败
  | "edited" // 上一轮改了文件
  | "idle"; // 兜底

export type Suggestion = {
  id: string;
  /** 芯片上显示的字，≤ 12 个汉字 */
  label: string;
  /** 点击后真正发出去的文本，可以比 label 完整 */
  text: string;
  source: SuggestionSourceId;
};

export type SuggestionInput = {
  events: SessionEvent[];
  sessionStatus: SessionStatus;
  /** 本次拖拽刚插入的路径；未拖拽为空数组 */
  droppedPaths: string[];
  /** 拖拽刚插入后的草稿快照，用于判定"草稿等于没写" */
  draft: string;
  draftAfterDrop: string | null;
};

const DOC_EXT = new Set([".md", ".txt", ".rst", ".adoc"]);
const CODE_EXT = new Set([
  ".ts",
  ".tsx",
  ".js",
  ".jsx",
  ".rs",
  ".py",
  ".go",
  ".java",
  ".c",
  ".cpp",
  ".cs",
  ".swift",
  ".kt",
  ".rb",
  ".php",
]);
const IMAGE_EXT = new Set([".png", ".jpg", ".jpeg", ".gif", ".webp", ".svg"]);
const CONFIG_EXT = new Set([".json", ".jsonc", ".yaml", ".yml", ".toml", ".ini", ".csv"]);

/** Feature flag — default on. */
export function suggestionsEnabled(): boolean {
  try {
    const v = window.localStorage.getItem("marionette-suggestions");
    if (v === "0" || v === "false" || v === "off") return false;
    return true;
  } catch {
    return true;
  }
}

export function setSuggestionsEnabled(on: boolean): void {
  try {
    window.localStorage.setItem("marionette-suggestions", on ? "on" : "off");
  } catch {
    /* ignore */
  }
}

/** "我帮你重构一下" → "重构一下"；"I can run the tests" → "run the tests" */
export function stripFirstPerson(action: string): string {
  let s = action.trim();
  // English first
  s = s.replace(/^(I can |I'll |I will |I' ll |me to |Let me |let me )/i, "");
  // Chinese — longer phrases first
  const zhPrefixes = ["我可以", "我帮你", "帮你", "让我", "我来", "我先", "我"];
  for (const p of zhPrefixes) {
    if (s.startsWith(p)) {
      s = s.slice(p.length);
      break;
    }
  }
  s = s.replace(/(吗|呢|？|\?|。)+$/g, "").trim();
  return s;
}

function clipLabel(text: string, max = 12): string {
  const t = text.trim();
  if ([...t].length <= max) return t;
  return `${[...t].slice(0, max - 1).join("")}…`;
}

function stripMarkdown(text: string): string {
  return text
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/\*([^*]+)\*/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/^#{1,6}\s+/gm, "")
    .replace(/^\s*[-*+]\s+/gm, "")
    .replace(/^\s*\d+\.\s+/gm, "");
}

function lastAssistantText(events: SessionEvent[]): string | null {
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i]!;
    if (e.type === "assistant_message" && e.text.trim()) return e.text;
  }
  return null;
}

function looksLikeClassifiedError(text: string): boolean {
  // formatClassifiedError → `**Title:** message`
  return (
    /^\*\*[^*]+:\*\*/.test(text.trim()) ||
    /\*\*(Agent error|Auth|Timeout|Network|Permission denied|Command not found|Model error):?\*\*/i.test(
      text,
    )
  );
}

/** Tail 200 chars of stripped assistant text, then sentences from the end. */
function tailQuestionSentence(assistantText: string): string | null {
  const stripped = stripMarkdown(assistantText);
  const tail = stripped.length > 200 ? stripped.slice(-200) : stripped;
  const parts = tail.split(/(?<=[。！？!?\n])/).map((s) => s.trim()).filter(Boolean);
  const questionHint =
    /[？?]$|吗|呢|要不要|是否|需要我|用不用|还是|should I|do you want|would you like|shall I|want me to/i;

  for (let i = parts.length - 1; i >= 0; i--) {
    const s = parts[i]!;
    if (questionHint.test(s)) return s;
  }
  return null;
}

function buildQuestionChips(sentence: string): Suggestion[] {
  const s = sentence.trim();

  // A 还是 B
  const orMatch = s.match(/^(.+?)还是(.+?)[？?吗呢。!！]*$/);
  if (orMatch && !/要不要|是否|需要我|用不用|should I|want me/i.test(s)) {
    const a = stripFirstPerson(orMatch[1]!.replace(/[，,、]/g, "").trim());
    const b = stripFirstPerson(orMatch[2]!.replace(/[，,、]/g, "").trim());
    if (a && b && [...a].length <= 40 && [...b].length <= 40) {
      return [
        { id: "q-or-a", label: clipLabel(a), text: a, source: "options" },
        { id: "q-or-b", label: clipLabel(b), text: b, source: "options" },
      ];
    }
  }

  // 要不要 X / 用不用 X
  let m = s.match(/(?:要不要|用不用)\s*(.+?)[？?吗呢。!！]*$/i);
  if (m) {
    const action = stripFirstPerson(m[1]!);
    if (action) {
      return [
        {
          id: "q-yes",
          label: clipLabel(action),
          text: t("suggestions.yesAction", { action }),
          source: "question",
        },
        { id: "q-no", label: t("suggestions.noThanks"), text: t("suggestions.noThanks"), source: "question" },
      ];
    }
  }

  // 需要我 X 吗 / 是否(需要) X
  m = s.match(/需要我\s*(.+?)[吗呢？?。!！]*$/i);
  if (m) {
    const action = stripFirstPerson(m[1]!);
    if (action) {
      return [
        {
          id: "q-yes",
          label: clipLabel(action),
          text: t("suggestions.yesAction", { action }),
          source: "question",
        },
        { id: "q-no", label: t("suggestions.notNow"), text: t("suggestions.notNow"), source: "question" },
      ];
    }
  }

  m = s.match(/是否(?:需要)?\s*(.+?)[？?吗呢。!！]*$/i);
  if (m) {
    const action = stripFirstPerson(m[1]!);
    if (action) {
      return [
        {
          id: "q-yes",
          label: clipLabel(action),
          text: t("suggestions.yesAction", { action }),
          source: "question",
        },
        { id: "q-no", label: t("suggestions.noThanks"), text: t("suggestions.noThanks"), source: "question" },
      ];
    }
  }

  // 我可以 X，要吗/好吗
  m = s.match(/(?:我可以|我能|我来)\s*(.+?)[，,]?\s*(?:要吗|好吗|可以吗)[？?。!！]*$/i);
  if (m) {
    const action = stripFirstPerson(m[1]!);
    if (action) {
      return [
        {
          id: "q-yes",
          label: clipLabel(action),
          text: t("suggestions.yesAction", { action }),
          source: "question",
        },
        { id: "q-no", label: t("suggestions.noThanks"), text: t("suggestions.noThanks"), source: "question" },
      ];
    }
  }

  // English: Should I X? / Want me to X? / Would you like me to X? / Do you want me to X?
  m = s.match(
    /(?:should I|shall I|want me to|would you like(?: me)? to|do you want(?: me)? to)\s+(.+?)[？?.!]*$/i,
  );
  if (m) {
    const action = stripFirstPerson(m[1]!);
    if (action) {
      return [
        {
          id: "q-yes",
          label: clipLabel(action),
          text: `Yes, ${action}`,
          source: "question",
        },
        { id: "q-no", label: "Not now", text: "Not now", source: "question" },
      ];
    }
  }

  // Question-ish but no actionable pattern — prefer silence over cheap 是/不是.
  return [];
}

/** Numbered / bulleted options immediately after a question. */
function buildOptionChips(assistantText: string): Suggestion[] | null {
  const stripped = stripMarkdown(assistantText);
  const tail = stripped.length > 400 ? stripped.slice(-400) : stripped;
  // Require a question-ish lead-in nearby
  if (!/[？?]|吗|呢|要不要|是否|which|choose|pick|option/i.test(tail)) return null;

  const lines = tail.split(/\r?\n/);
  const items: { n: number; text: string }[] = [];
  for (const line of lines) {
    const m = line.match(/^\s*(?:(\d+)[.)、]\s+|[-*+]\s+)(.+)$/);
    if (!m) continue;
    const text = m[2]!.trim();
    if (!text || [...text].length > 40) continue;
    items.push({ n: m[1] ? Number(m[1]) : items.length + 1, text });
  }
  if (items.length < 2) return null;
  return items.slice(0, 3).map((item, i) => ({
    id: `opt-${i}`,
    label: clipLabel(item.text),
    text: t("suggestions.pickOption", { n: item.n, text: item.text }),
    source: "options" as const,
  }));
}

function pathExt(path: string): string {
  const base = path.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return "";
  return base.slice(dot).toLowerCase();
}

function isDirPath(path: string): boolean {
  return /[/\\]$/.test(path) || !pathExt(path);
}

function buildDropChips(paths: string[]): Suggestion[] {
  if (paths.length === 0) return [];
  if (paths.length >= 2) {
    // Infer dominant type from first path for third chip
    const primary = buildDropChips([paths[0]!]).slice(0, 1);
    const multi: Suggestion[] = [
      {
        id: "drop-multi-1",
        label: t("suggestions.dropLookFiles"),
        text: t("suggestions.dropLookFiles"),
        source: "drop",
      },
      {
        id: "drop-multi-2",
        label: t("suggestions.dropCompare"),
        text: t("suggestions.dropCompare"),
        source: "drop",
      },
    ];
    if (primary[0]) {
      multi.push({ ...primary[0], id: "drop-multi-3", source: "drop" });
    }
    return multi.slice(0, 3);
  }

  const p = paths[0]!;
  const base = (p.replace(/[/\\]+$/, "").split(/[/\\]/).pop() ?? p).toLowerCase();
  const ext = pathExt(p);

  if (isDirPath(p) && !ext) {
    return [
      {
        id: "drop-d1",
        label: t("suggestions.dropLookDir"),
        text: t("suggestions.dropLookDir"),
        source: "drop",
      },
      {
        id: "drop-d2",
        label: t("suggestions.dropOutlineDir"),
        text: t("suggestions.dropOutlineDir"),
        source: "drop",
      },
    ];
  }
  if (DOC_EXT.has(ext)) {
    return [
      {
        id: "drop-doc1",
        label: t("suggestions.dropLookDoc"),
        text: t("suggestions.dropLookDoc"),
        source: "drop",
      },
      {
        id: "drop-doc2",
        label: t("suggestions.dropReviewDoc"),
        text: t("suggestions.dropReviewDoc"),
        source: "drop",
      },
      {
        id: "drop-doc3",
        label: t("suggestions.dropSummarizeDoc"),
        text: t("suggestions.dropSummarizeDoc"),
        source: "drop",
      },
    ];
  }
  if (CODE_EXT.has(ext)) {
    return [
      {
        id: "drop-code1",
        label: t("suggestions.dropLookFile"),
        text: t("suggestions.dropLookFile"),
        source: "drop",
      },
      {
        id: "drop-code2",
        label: t("suggestions.dropReviewCode"),
        text: t("suggestions.dropReviewCode"),
        source: "drop",
      },
      {
        id: "drop-code3",
        label: t("suggestions.dropExplainCode"),
        text: t("suggestions.dropExplainCode"),
        source: "drop",
      },
    ];
  }
  if (IMAGE_EXT.has(ext)) {
    return [
      {
        id: "drop-img1",
        label: t("suggestions.dropLookImage"),
        text: t("suggestions.dropLookImage"),
        source: "drop",
      },
      {
        id: "drop-img2",
        label: t("suggestions.dropImplementImage"),
        text: t("suggestions.dropImplementImage"),
        source: "drop",
      },
      {
        id: "drop-img3",
        label: t("suggestions.dropImageIssues"),
        text: t("suggestions.dropImageIssues"),
        source: "drop",
      },
    ];
  }
  if (ext === ".log" || /log|error|crash/i.test(base)) {
    return [
      {
        id: "drop-log1",
        label: t("suggestions.dropAnalyzeLog"),
        text: t("suggestions.dropAnalyzeLog"),
        source: "drop",
      },
      {
        id: "drop-log2",
        label: t("suggestions.dropFindErrorCause"),
        text: t("suggestions.dropFindErrorCause"),
        source: "drop",
      },
    ];
  }
  if (CONFIG_EXT.has(ext)) {
    return [
      {
        id: "drop-cfg1",
        label: t("suggestions.dropLookConfig"),
        text: t("suggestions.dropLookConfig"),
        source: "drop",
      },
      {
        id: "drop-cfg2",
        label: t("suggestions.dropCheckIssues"),
        text: t("suggestions.dropCheckIssues"),
        source: "drop",
      },
    ];
  }
  return [
    {
      id: "drop-unk",
      label: t("suggestions.dropLookFile"),
      text: t("suggestions.dropLookFile"),
      source: "drop",
    },
  ];
}

function turnHadError(events: SessionEvent[], sessionStatus: SessionStatus): boolean {
  if (sessionStatus === "error") return true;
  const text = lastAssistantText(events);
  if (text && looksLikeClassifiedError(text)) return true;
  return false;
}

/**
 * At most 3 chips. Returns empty when chips should not appear.
 * Priority: drop > options > question > error
 * (first non-empty source wins; sources are not mixed).
 *
 * No idle / "继续" fallback — empty is better than generic cheap chips.
 * Question only fires when we extract an actionable pattern (要不要 X / A 还是 B…).
 */
export function suggestFor(input: SuggestionInput): Suggestion[] {
  const { events, sessionStatus, droppedPaths, draft, draftAfterDrop } = input;

  if (sessionStatus === "running") return [];

  const draftIsEmpty =
    !draft.trim() ||
    (draftAfterDrop != null && draft === draftAfterDrop);

  if (!draftIsEmpty) return [];

  // drop — real files just inserted
  if (droppedPaths.length > 0 && draftAfterDrop != null && draft === draftAfterDrop) {
    const chips = buildDropChips(droppedPaths);
    if (chips.length) return chips.slice(0, 3);
  }

  const assistant = lastAssistantText(events);
  if (assistant) {
    const options = buildOptionChips(assistant);
    if (options?.length) return options.slice(0, 3);

    const sentence = tailQuestionSentence(assistant);
    if (sentence) {
      const q = buildQuestionChips(sentence);
      if (q.length) return q.slice(0, 3);
    }
  }

  // Real failure only — single useful action, no "换个思路" filler
  if (turnHadError(events, sessionStatus)) {
    return [
      { id: "err-retry", label: t("common.retry"), text: t("common.retry"), source: "error" },
    ];
  }

  // No edited / idle generics — silence when nothing concrete to suggest
  return [];
}

/** Stable key for "show once per (session, event tail)" dismissal. */
export function suggestionRoundKey(sessionId: string, events: SessionEvent[]): string {
  const last = events[events.length - 1];
  const stamp = last
    ? `${last.type}:${"createdAt" in last ? last.createdAt : ""}:${events.length}`
    : "empty";
  return `${sessionId}|${stamp}`;
}
