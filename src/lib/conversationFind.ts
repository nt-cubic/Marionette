/**
 * conversationFind — 对话内查找(当前会话,纯前端)。
 *
 * v1 范围:搜索**始终可见的主干卡**(你 / 回复 / 计划 / 提问 / 交接 /
 * 子代理 / 改动交付卡)。thought / tool_call 在运行段折叠簇内,簇的开合
 * 状态是卡片局部 state,外部命中后自动展开要穿多层 props——留待后续;
 * 计数口径因此是「可见消息命中」。
 *
 * 匹配在**已加载的转录范围**内进行(尾部 800 行 + 已翻页部分),纯函数、
 * 大小写不敏感子串。跳转/高亮在 SessionView(有虚拟列表的 offsetOf)。
 */

import type { SessionEvent } from "./types";

/** 参与查找的事件的可搜索文本;不参与查找的类型返回 null。 */
export function searchableText(event: SessionEvent): string | null {
  switch (event.type) {
    case "user_message":
    case "assistant_message":
      return event.text;
    case "plan_prompt":
      return [event.title, event.planMarkdown, event.decisionLabel].filter(Boolean).join("\n");
    case "ask_prompt":
      return [
        ...event.questions.flatMap((q) => [q.question, ...q.options.map((o) => o.label)]),
        ...(event.answers?.flatMap((a) => a.selected) ?? []),
      ].join("\n");
    case "handoff_prepared":
      return `${event.targetAgentId}\n${event.prompt}`;
    case "subtask_started":
      return `${event.agentLabel}\n${event.prompt}`;
    case "subtask_result":
      return [event.agentId, event.summary, event.error ?? ""].join("\n");
    case "file_change":
      return event.oldPath ? `${event.path}\n${event.oldPath}` : event.path;
    default:
      return null;
  }
}

/**
 * 事件流 → 命中的事件下标(升序)。空查询返回空数组;不做分词,
 * 子串即命中(与浏览器查找栏同语义)。
 */
export function findConversationMatches(
  events: readonly SessionEvent[],
  query: string,
): number[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const hits: number[] = [];
  for (let i = 0; i < events.length; i += 1) {
    const text = searchableText(events[i]!);
    if (text != null && text.toLowerCase().includes(needle)) hits.push(i);
  }
  return hits;
}
