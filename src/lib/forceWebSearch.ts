/**
 * Composer "force web search" toggle — not a network kill-switch.
 * Prepends a short hard instruction so agents that prefer offline solving
 * still use search/fetch tools when the user wants official docs first.
 */

export const FORCE_WEB_SEARCH_PREFIX = [
  "【联网检索】回答前请先使用你可用的网络搜索 / 网页抓取 / 文档查阅工具，",
  "查官方文档或权威来源；禁止仅凭训练记忆猜测 API、版本号、配置项或报错含义。",
  "检索完成后再作答，并简要写出依据来源（链接或文档名即可）。",
  "与当前项目仓库相关的仍可读本地文件，但外部事实以检索结果为准。",
  "",
  "---",
  "",
].join("");

/** Prepend the force-search instruction when the Composer toggle is on. */
export function withForceWebSearch(userText: string, enabled: boolean): string {
  if (!enabled) return userText;
  const body = userText.replace(/\r\n/g, "\n").trim();
  if (!body) return FORCE_WEB_SEARCH_PREFIX.trimEnd();
  // Avoid double-prefix if user resends / edit already includes it.
  if (body.startsWith("【联网检索】")) return userText;
  return `${FORCE_WEB_SEARCH_PREFIX}${body}`;
}

/**
 * Strip a previously prepended force-search block for display / re-edit.
 * Wire path should keep using {@link withForceWebSearch}; You cards show clean text.
 */
export function stripForceWebSearchPrefix(text: string): string {
  const t = text.replace(/\r\n/g, "\n");
  if (!t.startsWith("【联网检索】")) return text;
  const sep = "\n---\n\n";
  const i = t.indexOf(sep);
  if (i >= 0) return t.slice(i + sep.length);
  // Fallback: drop first line block until blank line
  const m = t.match(/^【联网检索】[\s\S]*?\n\n---\n\n?([\s\S]*)$/);
  return m ? m[1] : text;
}

export function detectForceWebSearchInText(text: string): boolean {
  return text.replace(/\r\n/g, "\n").startsWith("【联网检索】");
}
