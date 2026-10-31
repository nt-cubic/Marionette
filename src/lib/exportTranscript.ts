/**
 * exportTranscript — 会话导出为 Markdown(纯前端,零新依赖)。
 *
 * 数据源:load_transcript 的持久化事件(SessionEvent[],api.ts::loadTranscript
 * 注释里登记的「导出」全量消费者)。序列化规则:
 * - user_message / assistant_message 是对话主干,`---` 分隔轮次;
 * - thought / tool_call 收进 <details> 折叠块(GitHub/VS Code 预览均可渲染),
 *   对话主干保持可读;
 * - 连续 file_change 合成一张「本轮改动」表(交付卡口径:+/− 为 git 真数);
 * - plan/ask 卡带裁决结果,subagent/handoff 记一行 + 详情折叠。
 * 结构性文案走 i18n(导出物语言跟随导出时刻的 UI 语言)。
 */

import { t } from "./i18n";
import { loadTranscript } from "./api";
import type { SessionEvent } from "./types";

/** `HH:MM`(本地时间)。 */
function hhmm(iso: string): string {
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** `YYYY-MM-DD HH:mm`(本地时间)。 */
export function formatStamp(at: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

function details(summary: string, body: string): string {
  const trimmed = body.trim();
  if (!trimmed) return "";
  return `<details>\n<summary>${summary}</summary>\n\n${trimmed}\n\n</details>`;
}

function quietMeta(parts: Array<string | undefined>): string {
  const shown = parts.filter((p) => p && p.trim());
  return shown.length ? `*${shown.join(" · ")}*` : "";
}

function durationLabel(ms: number): string {
  return t("export.duration", { n: Math.max(1, Math.round(ms / 1000)) });
}

function changeLabel(changeType: string): string {
  switch (changeType) {
    case "added": return t("export.changeAdded");
    case "modified": return t("export.changeModified");
    case "deleted": return t("export.changeDeleted");
    default: return t("export.changeUntracked");
  }
}

function statusLabel(status: string): string {
  switch (status) {
    case "answered": return t("export.statusAnswered");
    case "declined": return t("export.statusDeclined");
    case "timeout": return t("export.statusTimeout");
    default: return t("export.statusPending");
  }
}

/** 连续 file_change 合成一张表;其余事件类型一行一段。 */
function fileChangeTable(events: Extract<SessionEvent, { type: "file_change" }>[]): string {
  const rows = events.map((e) => {
    const path = e.oldPath ? `${e.path} ← ${e.oldPath}` : e.path;
    const plus = e.binary || e.add === undefined ? "—" : String(e.add);
    const del = e.binary || e.del === undefined ? "—" : String(e.del);
    return `| ${path} | ${changeLabel(e.changeType)} | ${plus} | ${del} |`;
  });
  return [
    `**${t("export.changedFiles")}**`,
    "",
    `| ${t("export.file")} | ${t("export.change")} | + | − |`,
    "|---|---|---|---|",
    ...rows,
  ].join("\n");
}

/**
 * 事件流 → Markdown 文档。`sessionLabel` 作一级标题;结构文案按当前
 * locale 渲染。纯函数(时间读取本地时钟),verify 脚本可直接喂假事件。
 */
export function transcriptToMarkdown(events: readonly SessionEvent[], sessionLabel: string): string {
  const out: string[] = [];
  const talks = events.filter((e) => e.type === "user_message" || e.type === "assistant_message").length;
  out.push(`# ${sessionLabel}`, "", `> ${t("export.headerNote", { time: formatStamp() })} · ${t("export.eventCount", { count: events.length })} · ${t("export.turnCount", { count: talks })}`, "");
  let firstTurn = true;
  let fileRun: Extract<SessionEvent, { type: "file_change" }>[] = [];

  const flushFiles = () => {
    if (fileRun.length === 0) return;
    out.push(fileChangeTable(fileRun), "");
    fileRun = [];
  };

  for (const e of events) {
    switch (e.type) {
      case "user_message": {
        flushFiles();
        if (!firstTurn) out.push("---", "");
        firstTurn = false;
        out.push(`## ${t("export.you")} · ${hhmm(e.createdAt)}`, "");
        if (e.text.trim()) out.push(e.text.trim(), "");
        const meta = quietMeta([e.agentLabel, e.modelLabel, e.modeLabel, e.effortLabel]);
        if (meta) out.push(meta, "");
        for (const att of e.attachments ?? []) {
          out.push(`- 📎 ${att.name || att.path}`);
        }
        if (e.attachments?.length) out.push("");
        break;
      }
      case "assistant_message": {
        flushFiles();
        out.push(`## ${e.agentLabel || t("export.assistant")} · ${hhmm(e.createdAt)}`, "");
        if (e.text.trim()) out.push(e.text.trim(), "");
        const metaBits: string[] = [];
        if (e.modelLabel) metaBits.push(e.modelLabel);
        if (e.durationMs !== undefined) metaBits.push(durationLabel(e.durationMs));
        const meta = quietMeta(metaBits);
        if (meta) out.push(meta, "");
        break;
      }
      case "thought": {
        flushFiles();
        const block = details(t("export.thinking"), e.text);
        if (block) out.push(block, "");
        break;
      }
      case "tool_call": {
        flushFiles();
        const name = e.toolName || e.title || t("export.tool");
        const status = e.status ? ` (${e.status})` : "";
        const block = details(`${name}${status}`, e.text);
        if (block) out.push(block, "");
        break;
      }
      case "file_change": {
        fileRun.push(e);
        break;
      }
      case "handoff_prepared": {
        flushFiles();
        out.push(`> 🔗 ${t("export.handoff")} → ${e.targetAgentId} · ${e.handoffPath}`, "");
        const block = details(t("export.handoffPrompt"), e.prompt);
        if (block) out.push(block, "");
        break;
      }
      case "subtask_started": {
        flushFiles();
        out.push(`> 🧩 ${t("export.subagent")} · ${e.agentLabel}`, "");
        const block = details(t("export.subagentPrompt"), e.prompt);
        if (block) out.push(block, "");
        break;
      }
      case "subtask_result": {
        flushFiles();
        const dur = e.durationMs !== undefined ? ` · ${durationLabel(e.durationMs)}` : "";
        out.push(`> 🧩 ${t("export.subagent")} → ${e.status}${dur}`, "");
        const block = details(t("export.subagentSummary"), e.summary || e.error || "");
        if (block) out.push(block, "");
        break;
      }
      case "plan_prompt": {
        flushFiles();
        out.push(`### 📋 ${e.title || t("export.plan")}`, "");
        const block = details(t("export.planBody"), e.planMarkdown);
        if (block) out.push(block, "");
        out.push(`> ${e.decisionLabel || statusLabel(e.status)}`, "");
        break;
      }
      case "ask_prompt": {
        flushFiles();
        out.push(`### ❓ ${t("export.question")}`, "");
        e.questions.forEach((q, i) => {
          out.push(`${i + 1}. ${q.question}`);
          for (const opt of q.options) {
            const picked = e.answers?.some((a) => a.selected.includes(opt.label)) ? " ✓" : "";
            out.push(`   - ${opt.label}${picked}`);
          }
        });
        out.push("", `> ${statusLabel(e.status)}`, "");
        break;
      }
      default:
        break;
    }
  }
  flushFiles();
  return out.join("\n").replace(/\n{3,}/g, "\n\n").trimEnd() + "\n";
}

/** Windows 文件名净化 + 时间戳后缀:`修复导出 20261031-1630.md`。 */
export function exportFileName(sessionLabel: string, at: Date = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  const stamp = `${at.getFullYear()}${pad(at.getMonth() + 1)}${pad(at.getDate())}-${pad(at.getHours())}${pad(at.getMinutes())}`;
  const base = sessionLabel
    // 正则里用 \x22 表示引号——inventory-zh 的注释扫描器不识别正则字面量,
    // 裸引号会让它从此把注释当代码。
    .replace(/[\\/:*?<>|\x22]/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 60)
    .trimEnd() || "session";
  return `${base} ${stamp}.md`;
}

/** 触发 WebView2 原生下载(wry 未装 DownloadStarting handler → 默认下载 UI)。 */
export function downloadTextFile(fileName: string, content: string, mime = "text/markdown;charset=utf-8"): void {
  const blob = new Blob([content], { type: mime });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
}

/** 高层入口:拉全量转录 → 序列化 → 触发下载。返回给 toast 用的文件名与正文。 */
export async function exportSessionMarkdown(
  sessionId: string,
  sessionLabel: string,
): Promise<{ fileName: string; markdown: string }> {
  const events = (await loadTranscript(sessionId)) as SessionEvent[];
  const markdown = transcriptToMarkdown(events, sessionLabel);
  const fileName = exportFileName(sessionLabel);
  downloadTextFile(fileName, markdown);
  return { fileName, markdown };
}
