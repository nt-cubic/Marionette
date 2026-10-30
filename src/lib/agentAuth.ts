import { t } from "./i18n";

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
    get errorHint() {
      return t("auth.claudeCodeHint");
    },
  },
  codex: {
    probe: true,
    login: true,
    loginCommand: "codex login",
    get errorHint() {
      return t("auth.codexHint");
    },
  },
  "grok-build": {
    probe: true,
    login: true,
    loginCommand: "grok login",
    get errorHint() {
      return t("auth.grokHint");
    },
  },
  pi: {
    probe: true,
    login: true,
    loginCommand: "pi",
    get errorHint() {
      return t("auth.piHint");
    },
  },
  hermes: {
    probe: true,
    login: true,
    loginCommand: "hermes acp --setup",
    get errorHint() {
      return t("auth.hermesHint");
    },
  },
  opencode: {
    probe: true,
    login: true,
    loginCommand: "opencode auth login",
    get errorHint() {
      return t("auth.openCodeHint");
    },
  },
  deepseek: {
    probe: true,
    login: true,
    loginCommand: "dsh-acp login",
    get errorHint() {
      return t("auth.deepSeekHint");
    },
  },
  zcode: {
    probe: true,
    login: true,
    loginCommand: "zcode login",
    get errorHint() {
      return t("auth.zcodeHint");
    },
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
