/**
 * Claude Code ExitPlanMode arrives as session/request_permission with the plan
 * stuffed into toolCall.rawInput JSON. The UI must unwrap that markdown and
 * leave ordinary file-write permissions alone.
 *
 * Run: npx tsx scripts/verify-permission-plan.mts
 */
import assert from "node:assert/strict";
import {
  extractPermissionPlanMarkdown,
  isPlanApprovalTitle,
} from "../src/lib/permissionPlan.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const claudePlan = [
  "# Chrome/Edge 式标签页关闭体验",
  "",
  "## Context",
  "",
  "用户想要 Chrome/Edge 那样的标签行为：点 × 关闭一个标签后，下一个标签滑进来补位。",
].join("\n");

check("unwraps Claude Code {plan} JSON dump in detail", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Approve Plan",
    detail: JSON.stringify({ plan: claudePlan }),
    options: [
      { optionId: "yes", name: "Yes, manually approve edits", kind: "allow_once" },
      { optionId: "no", name: "No, keep planning", kind: "reject_once" },
    ],
  });
  assert.equal(md, claudePlan);
});

check("unwraps toolCall.rawInput.plan from params", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Approve Plan",
    detail: null,
    params: {
      toolCall: {
        title: "Approve Plan",
        rawInput: { plan: claudePlan },
      },
    },
  });
  assert.equal(md, claudePlan);
});

check("prefers explicit planMarkdown field", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Approve Plan",
    detail: JSON.stringify({ plan: "ignored" }),
    planMarkdown: claudePlan,
  });
  assert.equal(md, claudePlan);
});

check("accepts snake_case plan_content on raw_input", () => {
  const md = extractPermissionPlanMarkdown({
    params: {
      tool_call: {
        raw_input: { plan_content: "## Steps\n- a" },
      },
    },
  });
  assert.equal(md, "## Steps\n- a");
});

check("title-only markdown detail still counts as a plan", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Approve Plan",
    detail: "# Ready\n\nDo the thing.",
  });
  assert.equal(md, "# Ready\n\nDo the thing.");
});

check("ordinary file-write permission is not a plan", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Edit App.tsx",
    detail: JSON.stringify({
      path: "src/app/App.tsx",
      content: "export function App() {}",
    }),
    options: [
      { optionId: "allow-once", name: "Allow once", kind: "allow_once" },
      { optionId: "reject", name: "Reject", kind: "reject_once" },
    ],
  });
  assert.equal(md, null);
});

check("shell permission with a command is not a plan", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Run command",
    detail: "npm test",
    options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
  });
  assert.equal(md, null);
});

check("unwraps Codex PlanReview {plan} with Implement this plan? title", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Implement this plan?",
    params: {
      toolCall: {
        toolCallId: "plan-review:item-1",
        title: "Implement this plan?",
        kind: "switch_mode",
        rawInput: { plan: claudePlan },
      },
      options: [
        { optionId: "implement_plan", name: "Yes, implement this plan", kind: "allow_once" },
        {
          optionId: "revise_plan",
          name: "No, and tell Codex what to do differently",
          kind: "reject_once",
        },
      ],
    },
  });
  assert.equal(md, claudePlan);
});

check("isPlanApprovalTitle matches Claude Code and Codex titles", () => {
  assert.equal(isPlanApprovalTitle("Approve Plan"), true);
  assert.equal(isPlanApprovalTitle("Implement this plan?"), true);
  assert.equal(isPlanApprovalTitle("Edit App.tsx"), false);
  assert.equal(isPlanApprovalTitle("Yes, enter plan mode"), false);
});

check("keep-planning option + markdown detail is a plan", () => {
  const md = extractPermissionPlanMarkdown({
    title: "Tool permission required",
    detail: "# Ship it\n\n- step one",
    options: [{ optionId: "no", name: "No, keep planning", kind: "reject_once" }],
  });
  assert.equal(md, "# Ship it\n\n- step one");
});

console.log(`verify-permission-plan: ${passed} checks passed`);
