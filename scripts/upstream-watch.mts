// Upstream watch + port pipeline for the magpie engine seed.
//
// Watches yetone/magpie from a LOCAL research clone (never a GitHub fork, per
// the 2026-11-17 product decision), digests what changed against the engine
// contract (docs/plan-engine-contract.md), and — with --verify — actually
// ports a tag: imports it into an ISOLATED git worktree of this repo, builds
// the seed, and runs the contract suite against it. The main working tree is
// never mutated by this script; a green verify only means "ready to import".
//
// Usage:
//   npx tsx scripts/upstream-watch.mts              # fetch + digest new tags
//   npx tsx scripts/upstream-watch.mts --verify     # digest + port-verify latest
//   npx tsx scripts/upstream-watch.mts --verify v0.1.4332
// Clone location: $MARIONETTE_UPSTREAM_DIR or ../magpie-upstream (repo sibling).
// State/digests live in <clone>/.marionette-watch/ so nothing lands in the repo.
import { execFile as run } from "node:child_process";
import { promisify } from "node:util";
import { mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const execFile = promisify(run);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const UPSTREAM = "https://github.com/yetone/magpie.git";
const cloneDir = resolve(process.env.MARIONETTE_UPSTREAM_DIR ?? join(repoRoot, "..", "magpie-upstream"));
const watchDir = join(cloneDir, ".marionette-watch");
const digestDir = join(watchDir, "digests");
const stateFile = join(watchDir, "state.json");

const git = async (args: string[], cwd = cloneDir) =>
  (await execFile("git", args, { cwd, windowsHide: true, timeout: 300_000, maxBuffer: 64 * 1024 * 1024 })).stdout;

// ── contract areas: path prefix → {area, level} (docs/plan-engine-contract.md) ──
const AREAS: [string, string, "high" | "medium" | "low"][] = [
  ["internal/gateway/", "C2/C3/C5 gateway（body_cap 锚点风险）", "high"],
  ["internal/provider/", "C6/C4 provider（peel 调用点锚点风险）", "high"],
  ["plugins/groups/", "C4 组宣告", "high"],
  ["plugins/gateway/", "C5 网关插件", "high"],
  ["plugins/routing/", "C2/C5 路由", "high"],
  ["plugins/usage/", "C3 额度端点", "high"],
  ["internal/catalog/", "C2 预设目录", "medium"],
  ["internal/codexcat/", "C2 codex 目录", "medium"],
  ["internal/sqlitedb/", "存储", "medium"],
  ["internal/sessions/", "存储", "medium"],
  ["internal/appdir/", "C1 落盘布局", "medium"],
  ["internal/settings/", "C1 设置", "medium"],
  ["internal/pluginrt/", "C1 插件装载", "medium"],
  ["internal/plugin/", "C1 插件宿主", "medium"],
  ["main.go", "C7 启动序列/guard", "medium"],
  ["internal/cli/", "C7 种子旁路", "medium"],
  ["go.mod", "构建依赖", "medium"],
  ["go.sum", "构建依赖", "medium"],
  ["third_party/", "上游自带 vendored 依赖", "medium"],
];
const anchorRisk = (path: string) =>
  path.includes("internal/gateway/body_cap.go") ||
  path.includes("internal/gateway/body_cap") ||
  (path.startsWith("internal/provider/") && !path.startsWith("internal/provider/marionette_peel")) ||
  path === "main.go";

type State = { lastDigestTag?: string; verified: Record<string, string> };
async function loadState(): Promise<State> {
  if (!existsSync(stateFile)) return { verified: {} };
  return JSON.parse(await readFile(stateFile, "utf8")) as State;
}
async function saveState(s: State) {
  await mkdir(watchDir, { recursive: true });
  await writeFile(stateFile, JSON.stringify(s, null, 2) + "\n");
}

function tagNum(t: string): number {
  const m = /^v0\.1\.(\d+)$/.exec(t);
  return m ? Number(m[1]) : -1;
}

async function ensureClone() {
  if (!existsSync(join(cloneDir, ".git"))) {
    console.log(`[watch] cloning upstream into ${cloneDir} (local research clone, no fork) ...`);
    await execFile("git", ["clone", UPSTREAM, cloneDir], { windowsHide: true, timeout: 600_000, maxBuffer: 64 * 1024 * 1024 });
  }
  try {
    await git(["fetch", "--tags", "--prune", "origin"]);
  } catch (e) {
    console.log(`[watch] fetch failed (offline?): ${(e as Error).message.split("\n")[0]} — working from local tags`);
  }
}

async function allTags(): Promise<string[]> {
  const out = await git(["tag", "--list", "v0.1.*"]);
  return out.split("\n").map((t) => t.trim()).filter((t) => tagNum(t) >= 0).sort((a, b) => tagNum(a) - tagNum(b));
}

// ── digest ───────────────────────────────────────────────────────────────────
async function digest(from: string, to: string): Promise<string> {
  const stat = await git(["diff", "--stat", `${from}..${to}`]);
  const log = await git(["log", "--oneline", `${from}..${to}`]);
  const nameOnly = await git(["diff", "--name-only", `${from}..${to}`]);
  const files = nameOnly.split("\n").map((f) => f.trim()).filter(Boolean);

  const byArea = new Map<string, { level: string; label: string; files: string[] }>();
  for (const f of files) {
    const hit = AREAS.find(([prefix]) => f.startsWith(prefix) || f === prefix);
    const area = hit?.[1] ?? "种子不链接/未映射";
    const level = hit?.[2] ?? "low";
    if (!byArea.has(area)) byArea.set(area, { level, label: area, files: [] });
    byArea.get(area)!.files.push(f);
  }
  const risky = files.filter(anchorRisk);
  const commits = log.split("\n").filter(Boolean);
  let goMod = "";
  try {
    const gm = await git(["diff", `${from}..${to}`, "--", "go.mod"]);
    const added = gm.split("\n").filter((l) => /^\+\s*[a-z0-9./-]+\s+v/.test(l)).slice(0, 30);
    if (added.length) goMod = `\n### go.mod 新增/变更\n\`\`\`\n${added.join("\n")}\n\`\`\`\n`;
  } catch { /* go.mod untouched */ }

  const order = { high: 0, medium: 1, low: 2 } as Record<string, number>;
  const areas = [...byArea.entries()]
    .filter(([, v]) => v.level !== "low")
    .sort((a, b) => order[a[1].level] - order[b[1].level] || b[1].files.length - a[1].files.length);
  const lowCount = files.length - areas.reduce((n, [, v]) => n + v.files.length, 0);

  return `# magpie 上游移植简报 ${from} → ${to}

- 新版本数：${commits.length}（tag 数按 semver 计）
- 变更文件：${files.length} 个（其中 ${lowCount} 个不在合同映射内，种子不链接或未映射）
- 结论入口：合同清单 docs/plan-engine-contract.md；验证套件 scripts/verify-engine-contract.mts

## 按合同区分级

${areas.map(([area, v]) => `### ${v.level === "high" ? "🔴" : "🟡"} ${area}（${v.level}，${v.files.length} 文件）\n${v.files.slice(0, 25).map((f) => `- ${f}`).join("\n")}${v.files.length > 25 ? `\n- …${v.files.length - 25} more` : ""}`).join("\n\n")}${goMod}
## 补丁锚点风险（import-magpie 可能响亮失败）

${risky.length ? risky.map((f) => `- ⚠️ ${f}`).join("\n") : "- 无：本轮变更未触碰三个 Marionette 补丁的锚点文件"}

## 最近提交（最多 30 条）

${commits.slice(0, 30).map((c) => `- ${c}`).join("\n")}${commits.length > 30 ? `\n- …${commits.length - 30} more` : ""}

## 下一步

\`\`\`
npx tsx scripts/upstream-watch.mts --verify ${to}
\`\`\`
绿灯 = ready to import（主工作树仍不动，人工跑 tools\\build-portable.bat 前先提交 third_party/magpie）。
`;
}

// ── verify: port a tag in an isolated worktree, run the contract suite ──────
async function verify(tag: string): Promise<string> {
  // Short base name + core.longpaths: upstream ships a ~181-char testdata
  // fixture path (internal/gateway/testdata/qoder-*) that blows past
  // MAX_PATH otherwise — the same trap tools\import-magpie.ps1 documents
  // for its own temp clone (2026-11-12).
  const wt = join(await import("node:os").then((o) => o.tmpdir()), "mpv-" + tag.replace(/[^\w.]/g, ""));
  await rm(wt, { recursive: true, force: true }).catch(() => {});
  const report: string[] = [`# 移植验证 ${tag}`, ""];
  let verdict = "FAIL";
  try {
    console.log(`[verify] worktree: ${wt}`);
    await execFile("git", ["-c", "core.longpaths=true", "worktree", "add", "--detach", wt, "HEAD"],
      { cwd: repoRoot, windowsHide: true, timeout: 300_000, maxBuffer: 64 * 1024 * 1024 });
    console.log(`[verify] import-magpie ${tag} (isolated in the worktree) ...`);
    const imp = await execFile("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", join(wt, "tools", "import-magpie.ps1"), "-Tag", tag],
      { cwd: wt, windowsHide: true, timeout: 900_000, maxBuffer: 64 * 1024 * 1024 });
    report.push("## import-magpie", "```", imp.stdout.trim().split("\n").slice(-12).join("\n"), "```", "");
    console.log("[verify] building seed ...");
    const seedOut = join(wt, "engine-embedded-build");
    const { mkdtemp } = await import("node:fs/promises");
    const { tmpdir } = await import("node:os");
    const buildDir = await mkdtemp(join(tmpdir(), "marionette-verify-seed-"));
    const seedExe = join(buildDir, "magpie-seed.exe");
    await execFile("go", ["build", "-tags", "nogui marionette_seed", "-trimpath", "-ldflags", `-s -w -X main.version=${tag}`, "-o", seedExe, "."],
      { cwd: join(wt, "third_party", "magpie"), windowsHide: true, timeout: 600_000, maxBuffer: 64 * 1024 * 1024 });
    const seedBytes = (await import("node:fs")).statSync(seedExe).size;
    report.push("## 种子构建", `raw: ${seedBytes} bytes (${(seedBytes / 1048576).toFixed(1)} MB)`, "");
    console.log("[verify] contract suite against the built seed ...");
    // Node 24 type stripping runs .mts directly — no npx/.cmd dance (spawn
    // EINVAL on win32, ENOENT without the extension).
    const suite = await execFile(process.execPath, ["scripts/verify-engine-contract.mts", seedExe],
      { cwd: repoRoot, windowsHide: true, timeout: 600_000, maxBuffer: 64 * 1024 * 1024 });
    report.push("## 合同套件", "```", suite.stdout.trim(), "```", "");
    verdict = "PASS";
    await rm(buildDir, { recursive: true, force: true }).catch(() => {});
  } catch (e) {
    const err = e as { stdout?: string; stderr?: string; message?: string };
    report.push("## 失败", "```", (err.stdout ?? "").slice(-3000), (err.stderr ?? "").slice(-3000), err.message ?? "", "```", "");
    console.log(`[verify] FAILED: ${(err.message ?? "").split("\n")[0]}`);
  } finally {
    await execFile("git", ["worktree", "remove", "--force", wt], { cwd: repoRoot, windowsHide: true, timeout: 120_000 }).catch(() => {
      console.log(`[verify] warn: could not remove worktree ${wt} — remove it by hand`);
    });
  }
  report.push(`## 结论：${verdict}`, "");
  report.push(verdict === "PASS"
    ? `${tag} 通过全部合同测试。ready to import：主工作树跑 \`tools\\import-magpie.ps1 -Tag ${tag}\`（或让 build-portable 默认拉 latest）→ \`tools\\build-portable.bat\`。`
    : `${tag} 未通过。主工作树未被动过；修完再跑 \`--verify ${tag}\`。`);
  return report.join("\n");
}

// ── main ─────────────────────────────────────────────────────────────────────
const args = process.argv.slice(2);
const doVerify = args.includes("--verify");
const verifyArg = doVerify ? args[args.indexOf("--verify") + 1] : undefined;

await ensureClone();
const tags = await allTags();
if (!tags.length) { console.log("[watch] no v0.1.* tags found"); process.exit(1); }
const latest = tags[tags.length - 1];
const state = await loadState();

if (doVerify) {
  const tag = verifyArg && verifyArg.startsWith("v") ? verifyArg : latest;
  await mkdir(digestDir, { recursive: true });
  const file = join(digestDir, `verify-${tag}.md`);
  const report = await verify(tag);
  await writeFile(file, report);
  state.verified[tag] = verdictOf(report);
  await saveState(state);
  console.log(`[watch] verify report: ${file}`);
  console.log(report.split("\n").slice(-3).join("\n"));
  process.exit(state.verified[tag] === "PASS" ? 0 : 2);
}

// digest mode
await mkdir(digestDir, { recursive: true });
if (!state.lastDigestTag) {
  state.lastDigestTag = latest;
  await saveState(state);
  console.log(`[watch] baseline initialized at ${latest} (${tags.length} tags in the local clone)`);
  console.log(`[watch] next runs digest everything after ${latest}; port-verify with --verify`);
  process.exit(0);
}
const fromTag = state.lastDigestTag;
if (tagNum(latest) <= tagNum(fromTag)) {
  console.log(`[watch] no new tags since ${fromTag} (latest ${latest})`);
  process.exit(0);
}
const digestText = await digest(fromTag, latest);
const file = join(digestDir, `${fromTag}..${latest}.md`);
await writeFile(file, digestText);
state.lastDigestTag = latest;
await saveState(state);
console.log(`[watch] ${fromTag} → ${latest}: digest at ${file}`);
console.log(digestText.split("\n").filter((l) => l.startsWith("### ") || l.startsWith("- 新版本数")).join("\n"));

function verdictOf(report: string): string {
  return report.includes("## 结论：PASS") ? "PASS" : "FAIL";
}
