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
      "Grok 未登录时可点横幅 Sign in，完成后会自动重连；也可终端执行 `grok login`。令牌约 7 天过期。",
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
  opencode: {
    probe: true,
    login: true,
    loginCommand: "opencode auth login",
    errorHint:
      "OpenCode Zen（opencode/…）要单独登录；已有 Key 的模型用 opencode-go/… 或 deepseek/…。点 Sign in 后会自动重连。",
  },
  deepseek: {
    probe: true,
    login: true,
    loginCommand: "dsh-acp login",
    errorHint:
      "DeepSeek 未配置时可点横幅 Sign in，或在 Provider keys 里填 DEEPSEEK_API_KEY，或终端执行 `dsh-acp login`。",
  },
  zcode: {
    probe: true,
    login: true,
    loginCommand: "zcode login",
    errorHint:
      "ZCode 未登录时可点横幅 Sign in（走浏览器授权），或先安装并登录 ZCode 桌面版 —— 凭据共享自 ~/.zcode。",
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
