import { agentAuthSpec } from "./agentAuth";

export type AgentErrorKind =
  | "auth"
  | "auth_transient"
  | "upgrade"
  | "command_missing"
  | "timeout"
  | "model"
  | "network"
  | "permission"
  | "limit"
  | "archived"
  | "busy"
  | "offline"
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
  /** Current model id (`provider/model` for OpenCode) so auth hints match the live channel. */
  currentModel?: string | null;
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

  // Grok (and similar) 401s that say the session is still signed in. Must run
  // before the auth regex — the body contains `auth` / `login`.
  if (isAuthTransientMessage(message)) {
    return {
      kind: "auth_transient",
      title: "认证暂时不可用",
      message,
      actionHint: "账号仍在登录。点重试，或稍后再发；不必重新 Sign in。",
    };
  }

  if (
    /auth|login|unauthorized|401|not logged|authentication required|sign.?in|oauth|api.?key|missing.?key|credentials|not authenticated|token expired|re-?auth/i.test(
      message
    )
  ) {
    let hint = authHint;
    if (isBridgeNativeModelId(opts?.currentModel)) {
      hint = bridgeCredentialHint(message, agentLabel);
    } else if (opts?.agentId === "opencode") {
      hint = openCodeAuthHint(opts.currentModel, authHint);
    } else if (isGrok) {
      hint = grokApiKeySpawnHint(message) ?? authHint;
    }
    return {
      kind: "auth",
      title: "需要登录",
      message,
      actionHint: hint,
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

  if (isTurnInFlightError(raw)) {
    return {
      kind: "busy",
      title: "上一条指令还没跑完",
      message,
      actionHint: "等这一轮结束会自动发送；也可点停止（Esc）中断当前回合后再发。",
    };
  }

  if (isSessionNotRunningError(raw)) {
    return {
      kind: "offline",
      title: "Agent 进程已结束",
      message,
      actionHint: "点重试会重新拉起进程并把这段历史接上去，不必新开对话。",
    };
  }

  return {
    kind: "generic",
    title: "Agent 错误",
    message,
  };
}

/**
 * Marionette's ACP concurrency gate refusing a second prompt on one connection
 * (`acp.rs::send_prompt`: one prompt turn per connection). A *transient* state,
 * not an agent failure — the caller should wait for `turn/complete` and retry
 * (see `performSend` in App.tsx) instead of painting an "Agent 错误" card.
 */
function errorMessageOf(raw: unknown): string {
  return typeof raw === "string"
    ? raw
    : raw instanceof Error
      ? raw.message
      : typeof (raw as { message?: unknown } | null)?.message === "string"
        ? String((raw as { message: string }).message)
        : "";
}

export function isTurnInFlightError(raw: unknown): boolean {
  return /turn is already in flight/i.test(errorMessageOf(raw));
}

/**
 * The ACP process map no longer has this dialog — parked, crashed, or a
 * race with `stop`. Callers must respawn and retry rather than paint
 * "Agent 错误: ACP session is not running".
 */
export function isSessionNotRunningError(raw: unknown): boolean {
  const message = errorMessageOf(raw);
  return (
    /ACP session is not running/i.test(message) ||
    /ACP session is not initialized/i.test(message)
  );
}

export function formatClassifiedError(err: ClassifiedError): string {
  const lines = [`**${err.title}:** ${err.message}`];
  if (err.actionHint) lines.push(`\n${err.actionHint}`);
  return lines.join("");
}

/** Grok `error_type: auth_transient` plus “still signed in… no need to /login”. */
export function isAuthTransientMessage(message: string): boolean {
  if (/auth[_\s-]?transient/i.test(message)) return true;
  return (
    /still signed in/i.test(message) &&
    /no need to[\s\S]{0,80}\/?login/i.test(message)
  );
}

/**
 * Native ids the cross-use bridge injects into an agent's own model catalog —
 * grok/opencode/pi `marionette/<id>`, DSH `marionette::<id>`, zcode
 * `marionette\<id>`. Must stay in sync with Composer.tsx::isBridgeNativeModel.
 */
function isBridgeNativeModelId(id: string | null | undefined): boolean {
  const model = id?.trim() ?? "";
  return (
    model.startsWith("marionette/") ||
    model.startsWith("marionette::") ||
    model.startsWith("marionette\\")
  );
}

/**
 * Cross-use sessions route through the local model-sharing engine; an auth
 * error there is the ENGINE's upstream credential failing (e.g. an expired
 * OpenCode Go API key), never the agent's own login. Point at the credential
 * page instead of the agent Sign-in (misdirected 2026-10-23: an
 * `api_key_expired` from the engine showed a「ZCode 未登录」banner and sent
 * the user to re-login uselessly).
 */
function bridgeCredentialHint(message: string, agentLabel: string): string {
  const goKeyExpired =
    /opencode\s*go/i.test(message) && /expired|过期/i.test(message);
  const expiry = goKeyExpired
    ? "（上游是 OpenCode Go：这把 API Key 已在服务端过期，重新登录不会续期——去 OpenCode Console 换新 Key）"
    : "";
  return `当前模型走 Marionette 模型共享引擎（跨用）${expiry}。这个认证错误出在引擎的上游凭据，不是 ${agentLabel} 登录——在设置的凭据页更新对应渠道的 Key 后重试，或把模型换回 ${agentLabel} 原生渠道。`;
}

function openCodeAuthHint(
  currentModel: string | null | undefined,
  fallback: string
): string {
  const model = currentModel?.trim() ?? "";
  const slash = model.indexOf("/");
  const provider = slash > 0 ? model.slice(0, slash) : "";
  if (provider === "opencode") {
    return "当前模型是 OpenCode Zen（opencode/…）。点 Sign in 登录 Zen，或把模型换成 opencode-go/…、deepseek/… 这类已有 Key 的渠道。登录完成后会自动重连。";
  }
  if (provider) {
    return `当前模型走 ${provider}。点 Sign in 登录该渠道，或在 Provider keys 里填 Key。登录完成后会自动重连。`;
  }
  return fallback;
}

function grokApiKeySpawnHint(message: string): string | null {
  if (/auth:\s*apikey/i.test(message) && /auth_kind\s*=\s*none/i.test(message)) {
    return "当前会话按 API Key 启动但没有可用的 Key。点 Sign in 登录 grok.com（完成后会自动重连），或换一个走 API 的模型。";
  }
  return null;
}
