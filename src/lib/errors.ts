import { agentAuthSpec } from "./agentAuth";

export type AgentErrorKind =
  | "auth"
  | "command_missing"
  | "timeout"
  | "model"
  | "network"
  | "permission"
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

  if (
    /auth|login|unauthorized|401|not logged|authentication required|sign.?in|oauth|api.?key|missing.?key|credentials/i.test(
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

  if (/network|econnrefused|enotfound|dns|socket|fetch failed|connection refused|rate.?limit|429/i.test(message)) {
    return {
      kind: "network",
      title: "网络 / 配额",
      message,
      actionHint: "检查网络、代理与服务商状态；限流时稍后再试。",
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
