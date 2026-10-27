/**
 * Agent error classification: login vs transient vs provider channel.
 *
 * Run: npx tsx scripts/verify-agent-errors.mts
 */
import assert from "node:assert/strict";
import { classifyAgentError, isSessionNotRunningError } from "../src/lib/errors.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

console.log("classifyAgentError splits login from transient auth");

check("Grok auth_transient is not 需要登录", () => {
  const err = classifyAgentError(
    'Internal error: {"error_type":"auth_transient","message":"Your session is still signed in. Please retry. There is no need to run /login."}',
    { agentId: "grok-build", agentLabel: "Grok" },
  );
  assert.equal(err.kind, "auth_transient");
  assert.equal(err.title, "认证暂时不可用");
  assert.match(err.actionHint ?? "", /不必重新 Sign in/);
});

check("Grok still-signed-in sentence without error_type is transient", () => {
  const err = classifyAgentError(
    "Your session is still signed in. There is no need to run /login.",
    { agentId: "grok-build" },
  );
  assert.equal(err.kind, "auth_transient");
});

check("Grok real logout stays 需要登录", () => {
  const err = classifyAgentError("HTTP 401 Unauthorized: not logged in", {
    agentId: "grok-build",
  });
  assert.equal(err.kind, "auth");
  assert.equal(err.title, "需要登录");
});

check("Grok Auth: ApiKey with auth_kind=none stays login with reconnect hint", () => {
  const err = classifyAgentError("Auth: ApiKey auth_kind=none no auth context", {
    agentId: "grok-build",
  });
  assert.equal(err.kind, "auth");
  assert.match(err.actionHint ?? "", /自动重连/);
});

check("OpenCode Zen provider error points at opencode/… or a keyed model", () => {
  const err = classifyAgentError(
    "Authentication required: provider authentication required",
    {
      agentId: "opencode",
      currentModel: "opencode/deepseek-v4.1-flash-free",
    },
  );
  assert.equal(err.kind, "auth");
  assert.match(err.actionHint ?? "", /Zen/);
  assert.match(err.actionHint ?? "", /opencode-go/);
});

check("OpenCode keyed-provider error names that provider", () => {
  const err = classifyAgentError(
    "Authentication required: provider authentication required",
    {
      agentId: "opencode",
      currentModel: "opencode-go/deepseek-v4.1-flash",
    },
  );
  assert.equal(err.kind, "auth");
  assert.match(err.actionHint ?? "", /opencode-go/);
  assert.doesNotMatch(err.actionHint ?? "", /Zen/);
});

check("Claude Authentication required stays login", () => {
  const err = classifyAgentError("Authentication required", {
    agentId: "claude-code",
    agentLabel: "Claude Code",
  });
  assert.equal(err.kind, "auth");
  assert.match(err.actionHint ?? "", /claude auth login/);
});

check("ACP session is not running is offline, not Agent 错误", () => {
  const err = classifyAgentError("ACP session is not running");
  assert.equal(err.kind, "offline");
  assert.equal(err.title, "Agent 进程已结束");
  assert.equal(isSessionNotRunningError("ACP session is not running"), true);
  assert.equal(isSessionNotRunningError("ACP session is not initialized"), true);
  assert.equal(isSessionNotRunningError("turn is already in flight"), false);
});

check("Grok 426 still needs update, not login", () => {
  const err = classifyAgentError(
    "Upgrade required: CLI version is outdated. Run grok update. HTTP status 426",
    { agentId: "grok-build" },
  );
  assert.equal(err.kind, "upgrade");
  assert.equal(err.title, "需要更新");
});

console.log(`\n${passed} checks passed`);
