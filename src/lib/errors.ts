import { agentAuthSpec } from "./agentAuth";
import { t } from "./i18n";

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
      ? t("errors.authHintAgent", { agent: agentLabel })
      : t("errors.authHintGeneric"));
  const isGrok = opts?.agentId === "grok-build" || opts?.agentId === "grok";
  const upgradeHint = isGrok
    ? t("errors.upgradeHintGrok")
    : t("errors.upgradeHintGeneric");

  // Before auth: a 426 body can mention `grok update` without being a login failure.
  if (
    /upgrade required|\bstatus\s*426\b|http_status["']?\s*[:=]\s*426|cli version\b[\s\S]{0,80}outdated|\boutdated\b[\s\S]{0,60}\bupdate\b/i.test(
      message
    )
  ) {
    return {
      kind: "upgrade",
      title: t("errors.upgradeTitle"),
      message,
      actionHint: upgradeHint,
    };
  }

  // Grok (and similar) 401s that say the session is still signed in. Must run
  // before the auth regex — the body contains `auth` / `login`.
  if (isAuthTransientMessage(message)) {
    return {
      kind: "auth_transient",
      title: t("errors.authTransientTitle"),
      message,
      actionHint: t("errors.authTransientHint"),
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
      title: t("errors.authTitle"),
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
      title: t("errors.commandMissingTitle"),
      message,
      actionHint: t("errors.commandMissingHint"),
    };
  }

  if (/timeout|timed out|deadline|etimedout/i.test(message)) {
    return {
      kind: "timeout",
      title: t("errors.timeoutTitle"),
      message,
      actionHint: t("errors.timeoutHint"),
    };
  }

  if (
    /unknown config option|unsupported model|model not|invalid model|effort|config option/i.test(
      message
    )
  ) {
    return {
      kind: "model",
      title: t("errors.modelTitle"),
      message,
      actionHint: t("errors.modelHint"),
    };
  }

  if (/archiv/i.test(lower)) {
    return {
      kind: "archived",
      title: t("errors.archivedTitle"),
      message,
      actionHint: t("errors.archivedHint"),
    };
  }

  if (
    /rate.?limit|429|quota|usage.?limit|token.?limit|limit reached|out of credits|insufficient.?quota/i.test(
      message,
    )
  ) {
    return {
      kind: "limit",
      title: t("errors.limitTitle"),
      message,
      actionHint: t("errors.limitHint"),
    };
  }

  if (/network|econnrefused|enotfound|dns|socket|fetch failed|connection refused/i.test(message)) {
    return {
      kind: "network",
      title: t("errors.networkTitle"),
      message,
      actionHint: t("errors.networkHint"),
    };
  }

  if (/permission|denied|rejected|not allowed/i.test(lower) && !/auth/i.test(lower)) {
    return {
      kind: "permission",
      title: t("errors.permissionTitle"),
      message,
      actionHint: t("errors.permissionHint"),
    };
  }

  if (isTurnInFlightError(raw)) {
    return {
      kind: "busy",
      title: t("errors.busyTitle"),
      message,
      actionHint: t("errors.busyHint"),
    };
  }

  if (isSessionNotRunningError(raw)) {
    return {
      kind: "offline",
      title: t("errors.offlineTitle"),
      message,
      actionHint: t("errors.offlineHint"),
    };
  }

  return {
      kind: "generic",
      title: t("errors.genericTitle"),
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
    ? t("errors.bridgeGoKeyExpired")
    : "";
  return t("errors.bridgeCredentialHint", { agent: agentLabel, expiry });
}

function openCodeAuthHint(
  currentModel: string | null | undefined,
  fallback: string
): string {
  const model = currentModel?.trim() ?? "";
  const slash = model.indexOf("/");
  const provider = slash > 0 ? model.slice(0, slash) : "";
  if (provider === "opencode") {
    return t("errors.openCodeZenHint");
  }
  if (provider) {
    return t("errors.openCodeProviderHint", { provider });
  }
  return fallback;
}

function grokApiKeySpawnHint(message: string): string | null {
  if (/auth:\s*apikey/i.test(message) && /auth_kind\s*=\s*none/i.test(message)) {
    return t("errors.grokApiKeyHint");
  }
  return null;
}
