/**
 * Per-agent auth support registry: which agents Marionette can probe locally
 * and which have a native login flow it can launch. Drives the Clean banner,
 * the Sign in button and the per-agent error hints.
 */

export type AgentAuthSpec = {
  /** Marionette can detect logged-in/out state locally. */
  probe: boolean;
  /** Marionette can launch the agent's native login flow. */
  login: boolean;
  /** Terminal command the user can run as a fallback. */
  loginCommand: string;
  /** Sentence appended to an auth-classified turn error in Clean. */
  errorHint: string;
};

export const AGENT_AUTH: Record<string, AgentAuthSpec> = {
  "claude-code": {
    probe: true,
    login: true,
    loginCommand: "claude auth login",
    errorHint:
      "Claude 未登录时可点横幅 Sign in，或终端执行 `claude auth login` 后新建会话。",
  },
  codex: {
    probe: true,
    login: true,
    loginCommand: "codex login",
    errorHint:
      "Codex 未登录时可点横幅 Sign in，或终端执行 `codex login` 后新建会话。",
  },
  "grok-build": {
    probe: true,
    login: true,
    loginCommand: "grok login",
    errorHint:
      "Grok 未登录时可点横幅 Sign in，或终端执行 `grok login` 后新建会话（令牌约 7 天过期）。",
  },
  "kimi-code": {
    probe: true,
    login: true,
    loginCommand: "kimi login",
    errorHint:
      "Kimi 未登录时可点横幅 Sign in，或终端执行 `kimi login` 完成设备码登录后新建会话。",
  },
  cursor: {
    probe: true,
    login: true,
    loginCommand: "cursor-agent login",
    errorHint:
      "Cursor 未登录时可点横幅 Sign in，或终端执行 `cursor-agent login` 后新建会话。",
  },
  openclaw: {
    probe: true,
    login: true,
    loginCommand: "openclaw models auth login",
    errorHint:
      "OpenClaw 未登录时可点横幅 Sign in，或终端执行 `openclaw models auth login --provider <id>` 后新建会话。",
  },
  cline: {
    probe: true,
    login: true,
    loginCommand: "cline auth",
    errorHint: "Cline 未登录时可点横幅 Sign in，或终端执行 `cline auth` 后新建会话。",
  },
  gemini: {
    probe: true,
    login: true,
    loginCommand: "gemini",
    errorHint:
      "Gemini 未登录时可点横幅 Sign in，或启动 `gemini` 选择「Login with Google」后新建会话。",
  },
  pi: {
    probe: true,
    login: true,
    loginCommand: "pi",
    errorHint:
      "Pi 未登录时可点横幅 Sign in，或启动 `pi` 后在会话内运行 `/login` 后新建会话。",
  },
  hermes: {
    probe: true,
    login: true,
    loginCommand: "hermes acp --setup",
    errorHint:
      "Hermes 未登录时可点横幅 Sign in，或终端执行 `hermes acp --setup` 配置凭证后新建会话。",
  },
  codebuddy: {
    probe: true,
    login: true,
    loginCommand: "codebuddy",
    errorHint:
      "CodeBuddy 未登录时可点横幅 Sign in，或启动 `codebuddy` 后在会话内运行 `/login` 后新建会话。",
  },
  opencode: {
    probe: true,
    login: true,
    loginCommand: "opencode auth login",
    errorHint:
      "OpenCode 未登录时可点横幅 Sign in，或终端执行 `opencode auth login` 后新建会话。",
  },
};

export function agentAuthSpec(
  agentId: string | undefined | null
): AgentAuthSpec | undefined {
  return agentId ? AGENT_AUTH[agentId] : undefined;
}

export function agentSupportsLogin(agentId: string | undefined | null): boolean {
  return Boolean(agentAuthSpec(agentId)?.login);
}
