import { agentAuthSpec } from "./agentAuth";

export type AgentErrorKind =
  | "auth"
  | "upgrade"
  | "command_missing"
  | "timeout"
  | "model"
  | "network"
  | "permission"
  | "limit"
  | "archived"
  | "generic";

export type ClassifiedError = {
  kind: AgentErrorKind;
  title: string;
  message: string;
  /** Short action the user can take, if any. */
  actionHint?: string;
};

export type ClassifyAgentErrorOpts = {
  /** When set, auth errors use that agent’s Sign in / login-command hint. */
  agentId?: string | null;
  agentLabel?: string | null;
};

/**
 * Pull a human-readable error from an ACP JSON-RPC payload.
 *
 * Grok wraps the real reason as `error.message = "Internal error"` and puts
 * the CLI sentence in `error.data.message` (426 outdated, 401 logged out, …).
 * Prefer that nested sentence so the banner can tell login from “update me”.
 */
export function formatAcpRpcError(data: unknown): string | null {
  if (data == null) return null;
  if (typeof data === "string") return data;
  if (typeof data !== "object") return String(data);
  const root = data as Record<string, unknown>;
  const err = (
    root.error && typeof root.error === "object" ? root.error : root
  ) as Record<string, unknown>;
  const message =
    (typeof err.message === "string" && err.message) ||
    (typeof root.message === "string" && root.message) ||
    null;
  const dataObj =
    err.data && typeof err.data === "object"
      ? (err.data as Record<string, unknown>)
      : null;
  const nested =
    (dataObj && typeof dataObj.message === "string" && dataObj.message) ||
    (dataObj && typeof dataObj.details === "string" && dataObj.details) ||
    (typeof err.data === "string" ? err.data : null) ||
    null;
  if (
    nested &&
    (!message || /^internal error$/i.test(message.trim()) || message === nested)
  ) {
    return nested;
  }
  if (message && nested && !message.includes(nested)) return `${message}: ${nested}`;
  if (message) return message;
  if (nested) return nested;
  try {
    return JSON.stringify(data);
  } catch {
    return "Unknown agent error";
  }
}

/** Map agent / transport errors into a stable product taxonomy. */
export function classifyAgentError(
  raw: unknown,
  opts?: ClassifyAgentErrorOpts
): ClassifiedError {
  const message =
    typeof raw === "string"
      ? raw
      : raw instanceof Error
        ? raw.message
        : (() => {
            try {
              return JSON.stringify(raw);
            } catch {
              return String(raw);
            }
          })();

  const lower = message.toLowerCase();
  const agentLabel = opts?.agentLabel?.trim() || opts?.agentId || "agent";
  const spec = agentAuthSpec(opts?.agentId);
  const authHint =
    spec?.errorHint ??
    (opts?.agentId
      ? `点横幅 Sign in 登录 ${agentLabel}，或在终端运行对应登录命令后新建/重连会话。`
      : "点横幅 Sign in，或在终端运行该 agent 的登录命令。");
  const isGrok = opts?.agentId === "grok-build" || opts?.agentId === "grok";
  const upgradeHint = isGrok
    ? "Grok CLI 版本过低。点横幅「更新」运行 `grok update`，完成后新建会话。"
    : "这个 CLI 版本过低。按提示更新后新建会话。";

  // Before auth: a 426 body can mention `grok update` without being a login failure.
  if (
    /upgrade required|\bstatus\s*426\b|http_status["']?\s*[:=]\s*426|cli version\b[\s\S]{0,80}outdated|\boutdated\b[\s\S]{0,60}\bupdate\b/i.test(
      message
    )
  ) {
    return {
      kind: "upgrade",
      title: "需要更新",
      message,
      actionHint: upgradeHint,
    };
  }

  if (
    /auth|login|unauthorized|401|not logged|authentication required|sign.?in|oauth|api.?key|missing.?key|credentials|not authenticated|token expired|re-?auth/i.test(
      message
    )
  ) {
    return {
      kind: "auth",
      title: "需要登录",
      message,
      actionHint: authHint,
    };
  }

  if (
    /not found on path|enoent|command not found|is not recognized|program not found|spawn.*failed/i.test(
      message
    )
  ) {
    return {
      kind: "command_missing",
      title: "找不到 Agent 命令",
      message,
      actionHint:
        "打开 Composer 里的 agent 菜单 — 有已知 npm 包的可点 Install。",
    };
  }

  if (/timeout|timed out|deadline|etimedout/i.test(message)) {
    return {
      kind: "timeout",
      title: "超时",
      message,
      actionHint: "重试一次，或确认 agent 进程仍在响应。",
    };
  }

  if (
    /unknown config option|unsupported model|model not|invalid model|effort|config option/i.test(
      message
    )
  ) {
    return {
      kind: "model",
      title: "模型 / 配置不支持",
      message,
      actionHint: "换一个模型，或若该 agent 没有 effort 则清掉 strength。",
    };
  }

  if (/archiv/i.test(lower)) {
    return {
      kind: "archived",
      title: "会话已归档",
      message,
      actionHint:
        "Codex 把这条会话归档了。在终端运行 `codex unarchive <session-id>` 后再重连，或开一个新会话。",
    };
  }

  if (
    /rate.?limit|429|quota|usage.?limit|token.?limit|limit reached|out of credits|insufficient.?quota/i.test(
      message,
    )
  ) {
    return {
      kind: "limit",
      title: "额度用尽",
      message,
      actionHint: "等重置窗口过了再试，或换一个模型 / 账号。",
    };
  }

  if (/network|econnrefused|enotfound|dns|socket|fetch failed|connection refused/i.test(message)) {
    return {
      kind: "network",
      title: "网络",
      message,
      actionHint: "检查网络、代理与服务商状态后再试。",
    };
  }

  if (/permission|denied|rejected|not allowed/i.test(lower) && !/auth/i.test(lower)) {
    return {
      kind: "permission",
      title: "权限被拒绝",
      message,
      actionHint: "在权限弹窗中确认，或检查项目路径是否已授权。",
    };
  }

  return {
    kind: "generic",
    title: "Agent 错误",
    message,
  };
}

export function formatClassifiedError(err: ClassifiedError): string {
  const lines = [`**${err.title}:** ${err.message}`];
  if (err.actionHint) lines.push(`\n${err.actionHint}`);
  return lines.join("");
}
