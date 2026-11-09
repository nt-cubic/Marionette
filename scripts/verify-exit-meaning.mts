/**
 * verify-exit-meaning — 命令退出码人话（读自 codeg exit-meaning 的移植）
 *
 * lib/exitMeaning.ts 的保守判读：
 * 1. 答案类：grep/rg exit 1 无匹配（quiet 或全零计数）、test exit 1 不成立、
 *    diff/cmp exit 1 有差异、which/type 的 not found、ls/cat 的 no such file、
 *    git grep --quiet 无匹配 / git diff --exit-code 有差异 —— isExitAnswer
 *    为真（面板按「完成」着色）。
 * 2. 命名失败：127 命令不存在、126 无法执行、timeout 的 124 超时。
 * 3. 保守面：管道取最后一段、`&& echo` 回退到等它的命令、`cd && …` 的 setup
 *    判定、set -e / 子壳 / 替换 / 后台一律放弃（返回 null，按失败展示）；
 *    grep 打了非零计数行（有匹配却 exit 1）不是答案；shell 自述行
 *    （bash: … command not found）不冒充 which 的 not found。
 *
 * 跑法：npx tsx scripts/verify-exit-meaning.mts
 */

import {
  isExitAnswer,
  readCommandExit,
  signalOfExitCode,
} from "../src/lib/exitMeaning";

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

const exit = (code: number | null) => ({ code });

// ── 1. 答案类 ──────────────────────────────────────────────────────────
check(
  "grep exit 1 无输出 → 无匹配",
  readCommandExit("grep -r foo src/", exit(1), "") === "noMatch",
);
check(
  "rg exit 1 只有全零计数（-c）→ 无匹配",
  readCommandExit("rg -c foo .", exit(1), "a.ts:0\nb.ts:0\n") === "noMatch",
);
check(
  "grep exit 1 打了非零计数 → 不是答案（保持失败）",
  readCommandExit("grep -c foo .", exit(1), "a.ts:3\n") === null,
);
check(
  "test exit 1 → 不成立",
  readCommandExit("test -f x.txt", exit(1), "") === "false",
);
check(
  "[ -d x ] exit 1 → 不成立",
  readCommandExit("[ -d x ]", exit(1), "") === "false",
);
check(
  "diff exit 1 → 有差异",
  readCommandExit("diff a.txt b.txt", exit(1), "1c1\n< a\n---\n> b\n") === "differs",
);
check(
  "cmp exit 1 → 有差异",
  readCommandExit("cmp a.bin b.bin", exit(1), "a.bin b.bin differ: byte 5") === "differs",
);
check(
  "which exit 1 → 未找到",
  readCommandExit("which foo", exit(1), "which: no foo in (/usr/bin)") === "notFound",
);
check(
  "ls exit 2 只报 no such file → 未找到",
  readCommandExit("ls missing-dir", exit(2), "ls: cannot access 'missing-dir': No such file or directory") === "notFound",
);
check(
  "cat exit 1 混了别的输出 → 保持失败",
  readCommandExit("cat x.txt", exit(1), "some content\nls: cannot access 'x.txt': No such file or directory\n") === null,
);
check(
  "git grep --quiet exit 1 无输出 → 无匹配",
  readCommandExit("git grep -q todo -- src/", exit(1), "") === "noMatch",
);
check(
  "git diff --exit-code exit 1 → 有差异",
  readCommandExit("git diff --exit-code", exit(1), "") === "differs",
);
check(
  "git rev-parse --verify --quiet exit 1 → 未找到",
  readCommandExit("git rev-parse --verify --quiet HEAD~99", exit(1), "") === "notFound",
);
check(
  "git diff 但 git 自己报错 → 保持失败",
  readCommandExit("git diff --exit-code", exit(1), "fatal: not a git repository\n") === null,
);

// ── 2. 命名失败与信号 ──────────────────────────────────────────────────
check("127 → 命令不存在", readCommandExit("foo --bar", exit(127), "sh: foo: command not found") === "commandNotFound");
check("126 → 无法执行", readCommandExit("./script.sh", exit(126), "") === "notExecutable");
check("timeout 的 124 → 超时", readCommandExit("timeout 30 sleep 60", exit(124), "") === "timedOut");
check("gtimeout 的 124 → 超时", readCommandExit("gtimeout 30 cargo test", exit(124), "") === "timedOut");
check("输出自述超时的 124 → 超时", readCommandExit("cargo test", exit(124), "error: test timed out after 60s") === "timedOut");
check("137 → SIGKILL", signalOfExitCode(137) === "SIGKILL");
check("143 → SIGTERM", signalOfExitCode(143) === "SIGTERM");
check("42 → 无信号名", signalOfExitCode(42) === null);

// ── 3. 保守面 ──────────────────────────────────────────────────────────
check(
  "管道取最后一段：cat file | grep x exit 1 → 无匹配",
  readCommandExit("cat big.log | grep ERROR", exit(1), "") === "noMatch",
);
check(
  "管道最后一段是 wc（0 也能退）→ 保持失败",
  readCommandExit("ls | grep missing | wc -l", exit(1), "") === null,
);
check(
  "`&& echo` 回退：grep && echo found 的 1 → 无匹配",
  readCommandExit("grep -q foo bar.txt && echo found", exit(1), "") === "noMatch",
);
check(
  "cd setup：cd dir && grep x file 的 1（无输出）→ 无匹配",
  readCommandExit("cd sub && grep x file.txt", exit(1), "") === "noMatch",
);
check(
  "cd setup 但有输出（可能是 cd 的抱怨）→ 保持失败",
  readCommandExit("cd sub && grep x file.txt", exit(1), "sh: cd: sub: No such file or directory") === null,
);
check(
  "未知 setup（rm && grep）→ 保持失败",
  readCommandExit("rm -f tmp && grep x file.txt", exit(1), "") === null,
);
check(
  "set -e 改变归属 → 放弃",
  readCommandExit("set -e; grep -q foo bar", exit(1), "") === null,
);
check(
  "命令替换 $( ) → 放弃",
  readCommandExit("echo $(grep foo bar)", exit(1), "") === null,
);
check(
  "子壳 → 放弃",
  readCommandExit("(grep foo bar)", exit(1), "") === null,
);
check(
  "后台 & → 放弃",
  readCommandExit("grep foo bar &", exit(1), "") === null,
);
check(
  "heredoc → 放弃",
  readCommandExit("cat <<EOF\ngrep foo\nEOF", exit(1), "") === null,
);
check(
  "exit 0 → 无含义",
  readCommandExit("grep foo bar", exit(0), "match") === null,
);
check(
  "exit 1 的 grep 但 shell 在抱怨 glob → 保持失败",
  readCommandExit("grep foo *.rs", exit(1), "zsh:1: no matches found: *.rs") === null,
);
check(
  "agent 框架行不算输出：Exit code 1 行被剥离后 quiet → 无匹配",
  readCommandExit("rg pattern src/", exit(1), "Exit code 1\n") === "noMatch",
);
check("isExitAnswer 只认答案类", isExitAnswer("noMatch") && isExitAnswer("differs") && !isExitAnswer("commandNotFound") && !isExitAnswer(null));

// ── 4. 面板聚合（backgroundTasks 消费同一模块） ─────────────────────────
// Node ≥24 的 navigator 取 OS 语言（本机 ja-JP → en），先钉住 zh 再断言文案。
const { applyLocale, t } = await import("../src/lib/i18n");
await applyLocale("zh-CN");
const detailCases: Array<[string, number, string, string]> = [
  ["grep -r foo src/", 1, "", "无匹配 · 退出码 1"],
  ["definitely-not-a-cmd", 127, "sh: definitely-not-a-cmd: command not found", "命令不存在 · 退出码 127"],
];
for (const [cmd, code, out, want] of detailCases) {
  const meaning = readCommandExit(cmd, exit(code), out);
  const got = meaning ? t("tasks.exitMeaning.withCode", { meaning: t(`tasks.exitMeaning.${meaning}`), code }) : "";
  check(`i18n 详情：${cmd} → ${want}`, got === want, got);
}

if (failures > 0) {
  console.error(`\n${failures} 项失败`);
  process.exit(1);
}
console.log("\nexit-meaning 全部通过");
