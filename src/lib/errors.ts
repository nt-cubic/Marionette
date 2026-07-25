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

/** Map agent / transport errors into a stable product taxonomy. */
export function classifyAgentError(raw: unknown): ClassifiedError {
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

  if (
    /auth|login|unauthorized|401|not logged|authentication required|sign.?in/i.test(message)
  ) {
    return {
      kind: "auth",
      title: "Sign in required",
      message,
      actionHint: "Use Sign in, or run the agent’s login command in a terminal.",
    };
  }

  if (
    /not found on path|enoent|command not found|is not recognized|program not found|spawn.*failed/i.test(
      message
    )
  ) {
    return {
      kind: "command_missing",
      title: "Agent command missing",
      message,
      actionHint:
        "Open the agent menu in the composer — agents with a known package have an Install button.",
    };
  }

  if (/timeout|timed out|deadline|etimedout/i.test(message)) {
    return {
      kind: "timeout",
      title: "Timed out",
      message,
      actionHint: "Retry, or check that the agent process is responding.",
    };
  }

  if (
    /unknown config option|unsupported model|model not|invalid model|effort|config option/i.test(
      message
    )
  ) {
    return {
      kind: "model",
      title: "Model / config not supported",
      message,
      actionHint: "Pick another model or clear effort if this agent does not expose it.",
    };
  }

  if (/network|econnrefused|enotfound|dns|socket|fetch failed|connection refused/i.test(message)) {
    return {
      kind: "network",
      title: "Network error",
      message,
      actionHint: "Check connectivity and provider status.",
    };
  }

  if (/permission|denied|rejected|not allowed/i.test(lower) && !/auth/i.test(lower)) {
    return {
      kind: "permission",
      title: "Permission denied",
      message,
    };
  }

  return {
    kind: "generic",
    title: "Agent error",
    message,
  };
}

export function formatClassifiedError(err: ClassifiedError): string {
  const lines = [`**${err.title}:** ${err.message}`];
  if (err.actionHint) lines.push(`\n${err.actionHint}`);
  return lines.join("");
}
