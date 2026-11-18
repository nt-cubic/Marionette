/**
 * verify-memo-box — 收纳盒交互梳理（2026-10-26）
 *
 * 1. splitShelfChatSessions：origin "memo" 的 Chat 会话拆进侧栏「收纳盒」
 *    栏目，其余（含无 origin 的旧数据行）留在「聊天」；非 Chat 项目不进任何组。
 * 2. isLegacyGhostProject / withoutGhostProjects：10-22 悬浮球方案的
 *    ~/.marionette/inbox 幽灵项目（正/反斜杠、尾斜杠）在加载口被过滤，
 *    真实项目一个不动。
 * 3. memoPrompt：dialog / doc 两种意图的预填文案契约（doc 指到项目
 *    docs/notes/<日期>；两类 intent 都逐条列出材料）。
 * 4. 文字拖放通道（vendor wry 补丁）。
 * 5. 帖子流：groupMemosByDay / memoLinkHost，以及 MemoBox 源码契约
 *    （顶上写一条 + 四种卡，不再渲染 40px 文件行）。
 * 6. 图片卡：横竖比 contain（memoImageOrient），整张图可见，不 object-cover 裁切。
 * 7. 2026-11-18 批：置顶分组 / 标签筛选（filterMemoEntries·splitPinnedGroup·
 *    memoTagVocabulary）/ 全卡拖拽（encode/decode 往返、Composer 先分流再走
 *    路径启发式）/ 输入框存盒入口。
 *
 * 跑法：npx tsx scripts/verify-memo-box.mts
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { CHAT_PROJECT_ID } from "../src/lib/api";
import { groupMemosByDay, memoImageOrient, memoLinkHost, memoPrompt } from "../src/lib/memos";
import {
  decodeMemoDrag,
  encodeMemoDrag,
  filterMemoEntries,
  memoDragText,
  memoTagVocabulary,
  splitPinnedGroup,
} from "../src/lib/memos";
import { installPageDragSourceTracker, isPageDragSource } from "../src/lib/dragText";
import { DOCK_DROP_PATHS_EVENT, DOCK_DROP_TEXT_EVENT, dispatchDockDropText } from "../src/lib/dockDrop";
import { isLegacyGhostProject, splitShelfChatSessions, withoutGhostProjects } from "../src/lib/sessionGroups";
import type { MemoEntry, MemoKind } from "../src/lib/memos";
import type { Project, Session } from "../src/lib/types";

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  if (ok) {
    console.log(`  ok  ${name}`);
  } else {
    failures += 1;
    console.error(`FAIL  ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

function session(partial: Partial<Session> & { id: string }): Session {
  return {
    projectId: CHAT_PROJECT_ID,
    agentId: "opencode",
    label: partial.id,
    cwd: "D:\\tmp",
    status: "exited",
    processId: null,
    startedAt: "",
    lastActiveAt: new Date().toISOString(),
    transcriptPath: "",
    handoffPath: "",
    viewMode: "clean",
    ...partial,
  };
}

// ── 1. 侧栏分组 ───────────────────────────────────────────────────────
{
  const split = splitShelfChatSessions([
    session({ id: "chat-plain" }),
    session({ id: "chat-legacy", origin: undefined }),
    session({ id: "memo-1", origin: "memo" }),
    session({ id: "memo-2", origin: "memo", label: "整理 · 攻略" }),
    session({ id: "staff-1", origin: "chief" }),
    session({ id: "proj-1", projectId: "project-xyz" }),
    session({ id: "imported", origin: "import" }),
  ]);
  check(
    "origin memo → 收纳盒组",
    split.memo.map((s) => s.id).join(",") === "memo-1,memo-2",
    JSON.stringify(split.memo.map((s) => s.id)),
  );
  check(
    "origin chief → 员工组，不进聊天",
    split.staff.map((s) => s.id).join(",") === "staff-1",
    JSON.stringify(split.staff.map((s) => s.id)),
  );
  check(
    "无 origin / user / import 的 Chat 行都留在聊天组",
    split.chat.map((s) => s.id).join(",") === "chat-plain,chat-legacy,imported",
    JSON.stringify(split.chat.map((s) => s.id)),
  );
  check("非 Chat 项目的会话不进任何组", split.staff.length + split.chat.length + split.memo.length === 6);
}

// ── 2. 幽灵 inbox 项目过滤 ────────────────────────────────────────────
{
  const project = (id: string, rootPath: string): Project => ({
    id,
    name: id,
    rootPath,
    createdAt: "",
    lastOpenedAt: "",
  });
  check(
    "本机实锤的幽灵条目被识别",
    isLegacyGhostProject(project("project-2f1b6269e6cbc931", "C:\\Users\\user\\.marionette\\inbox")),
  );
  check("正斜杠拼写也识别", isLegacyGhostProject(project("g2", "/home/u/.marionette/inbox")));
  check("尾斜杠也识别", isLegacyGhostProject(project("g3", "C:\\Users\\u\\.marionette\\inbox\\")));
  check("正常项目不误伤", !isLegacyGhostProject(project("p1", "D:\\Projects\\Marionette")));
  check(
    "工作区里的同名目录不误伤（不在 .marionette 数据目录下）",
    !isLegacyGhostProject(project("p2", "D:\\Projects\\inbox")),
  );
  const kept = withoutGhostProjects([
    project("ghost", "C:\\Users\\u\\.marionette\\inbox"),
    project("real", "D:\\code\\app"),
  ]);
  check("过滤后只剩真实项目", kept.length === 1 && kept[0].id === "real");
}

// ── 3. 预填文案 ───────────────────────────────────────────────────────
{
  const entries: MemoEntry[] = [
    { id: "a", kind: "link", title: "karakeep", content: "https://github.com/karakeep-app/", createdAt: "1" },
    { id: "b", kind: "text", title: "摘录一", content: "第一行\n第二行", createdAt: "2" },
    { id: "c", kind: "image", title: "图.png", path: "C:\\m\\files\\x.png", createdAt: "3" },
    { id: "d", kind: "file", title: "spec.pdf", path: "C:\\m\\files\\y.pdf", createdAt: "4" },
  ];
  const dialog = memoPrompt(entries, "dialog");
  check("dialog 开头点名材料数", dialog.startsWith("参考下面 4 条材料："));
  check("dialog 逐条列出链接 URL 与文件路径", dialog.includes("https://github.com/karakeep-app/") && dialog.includes("C:\\m\\files\\y.pdf"));

  const doc = memoPrompt(entries, "doc");
  check("doc 要求落到 docs/notes/<当日日期>", /docs\/notes\/\d{4}-\d{2}-\d{2}-<主题>\.md/.test(doc));
  check("doc 也带全部材料", doc.includes("图.png") || doc.includes("C:\\m\\files\\x.png"));

  // 类型完整性：四种 kind 都能出文案（不抛异常即通过）
  const kinds: MemoKind[] = ["link", "text", "image", "file"];
  check(
    "四种 kind 均可成文",
    kinds.every((kind) => {
      const one = memoPrompt([{ id: "k", kind, title: kind, content: "x", path: "p", createdAt: "1" }], "dialog");
      return one.includes("参考下面 1 条材料：");
    }),
  );
}

// ── 4. 文字拖放通道（vendor wry 补丁）─────────────────────────────────
// 页内 HTML5 拖拽必须留在纯 HTML5 链路：tracker 把「本页是拖拽源」标记出来，
// 所有 marionette-drag-text 监听据此让路；dock 路由的 chat 区文字经 DOM 事件转交。
{
  // dragText/dockDrop 都在调用时读 window —— 先装一个最小假 window。
  const fakeWindow = new EventTarget();
  (globalThis as Record<string, unknown>).window = fakeWindow;

  check("无拖拽时不是页内拖拽源", isPageDragSource() === false);
  const dispose = installPageDragSourceTracker();
  fakeWindow.dispatchEvent(new Event("dragstart"));
  check("dragstart 后标记页内拖拽源", isPageDragSource() === true);
  fakeWindow.dispatchEvent(new Event("dragend"));
  check("dragend 复位", isPageDragSource() === false);
  fakeWindow.dispatchEvent(new Event("dragstart"));
  fakeWindow.dispatchEvent(new Event("drop"));
  check("drop 也复位（源元素中途移除的兜底）", isPageDragSource() === false);
  dispose();
  fakeWindow.dispatchEvent(new Event("dragstart"));
  check("卸载后不再标记", isPageDragSource() === false);
  fakeWindow.dispatchEvent(new Event("dragend"));

  check("文件路径 DOM 事件名不变", DOCK_DROP_PATHS_EVENT === "marionette:dock-drop-paths");
  check("chat 区文字 DOM 事件就位", DOCK_DROP_TEXT_EVENT === "marionette:dock-drop-text");
  let received = "";
  fakeWindow.addEventListener(DOCK_DROP_TEXT_EVENT, (event) => {
    received = (event as CustomEvent<string>).detail;
  });
  dispatchDockDropText("https://example.com/a");
  check("dispatchDockDropText 携带原文", received === "https://example.com/a");
}

// ── 5. 帖子流（单列卡 + 顶上写一条）──────────────────────────────────
{
  const now = new Date(2026, 9, 26, 15, 0, 0).getTime(); // local 2026-10-26 15:00
  const stamp = (y: number, m: number, d: number, h = 12) =>
    String(new Date(y, m - 1, d, h).getTime());
  const groups = groupMemosByDay(
    [
      { id: "t1", kind: "text", title: "今早", createdAt: stamp(2026, 10, 26, 9) },
      { id: "t2", kind: "link", title: "中午", createdAt: stamp(2026, 10, 26, 12) },
      { id: "y1", kind: "image", title: "昨", createdAt: stamp(2026, 10, 25, 18) },
      { id: "w1", kind: "file", title: "上周", createdAt: stamp(2026, 10, 20, 10) },
      { id: "o1", kind: "text", title: "去年", createdAt: stamp(2025, 12, 1, 10) },
    ],
    now,
  );
  check(
    "同一天并进一组、倒序分段",
    groups.map((g) => `${g.label}:${g.entries.map((e) => e.id).join("+")}`).join("|") ===
      "今天:t1+t2|昨天:y1|10月20日:w1|2025年12月1日:o1",
    groups.map((g) => `${g.label}:${g.entries.map((e) => e.id).join("+")}`).join("|"),
  );
  check("链接 host 去掉 www", memoLinkHost("https://www.github.com/karakeep-app/karakeep") === "github.com");
  check("非 URL 原文回退", memoLinkHost("not a url") === "not a url");

  const here = dirname(fileURLToPath(import.meta.url));
  const box = readFileSync(join(here, "../src/components/MemoBox.tsx"), "utf8");
  const css = readFileSync(join(here, "../src/styles/dock.css"), "utf8");
  // Compose card copy lives in the i18n fragment since the 2026-10-30 conversion.
  check("顶上有写一条输入卡", box.includes("memo-box__compose") && (box.includes('t("memo.composePlaceholder")') || box.includes('t("v2memo.compose")')));
  check(
    "四种帖子卡都在",
    ["memo-box__card--text", "memo-box__card--image", "memo-box__card--link", "memo-box__card--file"].every((cls) =>
      box.includes(cls),
    ),
  );
  check("不再渲染 40px 文件行", !box.includes("memo-box__row") && !css.includes(".memo-box__row"));
  check("Ctrl+Enter 保存写下的内容", (box.includes("memo.composePlaceholder") || box.includes("v2memo.compose")) && box.includes('event.key !== "Enter"'));
  const shelf = readFileSync(join(here, "../src/components/ProjectShelf.tsx"), "utf8");
  check(
    "收纳盒入口常显数量",
    shelf.includes("shelf-memo-entry__count") &&
      shelf.includes("memoCount") &&
      /aria-label=\{`\$\{t\("shelf.memoBox"\)\} \$\{memoCount\}`\}/.test(shelf),
  );
  const panel = readFileSync(join(here, "../src/components/ContextPanel.tsx"), "utf8");
  check(
    "MCP/技能超过 8 条走显示更多",
    panel.includes("LEND_LIST_PREVIEW = 8") &&
      panel.includes("LendClip") &&
      panel.includes("shelf-clip--lend"),
  );
  check(
    "插件卡只剩统一入口（DSH 专属块已删，2026-11-06）",
    panel.includes('id="context-plugins"') &&
      panel.includes("UniversalPluginsSection embedded") &&
      // 词边界：UniversalPluginsSection 不算命中。
      !/\bPluginsSection\b/.test(panel),
  );

  const tabs = readFileSync(join(here, "../src/components/SessionView.tsx"), "utf8");
  check(
    "收纳盒选中时对话标签不再 is-active",
    // extraOwnsStrip 泛化后五种伪标签(memo/council/usage/doc/browser)共用;
    // memo 必须仍在条件里,usage 是 2026-11-01 补进来的。
    tabs.includes("extraOwnsStrip") &&
      tabs.includes("openSession.id === session.id && !extraOwnsStrip") &&
      tabs.includes("memoTab?.active ||") &&
      tabs.includes("usageTab?.active ||"),
  );
}

// ── 6. 图片卡按横竖比完整显示 ──────────────────────────────────
{
  check("16:9 是横图", memoImageOrient(1920, 1080) === "landscape");
  check("3:2 是横图", memoImageOrient(1500, 1000) === "landscape");
  check("5:4 是横图", memoImageOrient(1280, 1024) === "landscape");
  check("9:16 是竖图", memoImageOrient(1080, 1920) === "portrait");
  check("2:3 是竖图", memoImageOrient(800, 1200) === "portrait");
  check("4:5 是竖图", memoImageOrient(1080, 1350) === "portrait");
  check("1:1 是方图", memoImageOrient(800, 800) === "square");
  check("6:5 仍是方图", memoImageOrient(1200, 1000) === "square");
  check("坏尺寸回落方图", memoImageOrient(0, 10) === "square" && memoImageOrient(-1, 20) === "square");

  const here = dirname(fileURLToPath(import.meta.url));
  const box = readFileSync(join(here, "../src/components/MemoBox.tsx"), "utf8");
  const css = readFileSync(join(here, "../src/styles/dock.css"), "utf8");
  const shot = css.match(/\.memo-box__shot\s*\{[^}]+\}/);
  check(
    "图片卡走 contain 盒子，不再定高裁切",
    Boolean(shot) &&
      shot![0].includes("object-fit: contain") &&
      !shot![0].includes("object-fit: cover") &&
      !/height:\s*160px/.test(shot![0]) &&
      css.includes(".memo-box__shot-wrap") &&
      box.includes("memo-box__shot-wrap") &&
      box.includes("applyShotOrient") &&
      box.includes("dataset.orient"),
    shot?.[0] ?? "missing .memo-box__shot",
  );
  check("竖图有更高的上限", css.includes('data-orient="portrait"') && css.includes("36rem"));
}

// ── 7. 置顶 / 标签 / 全卡拖拽 / 输入框存盒（2026-11-18 批） ────────
{
  const entry = (id: string, partial: Partial<MemoEntry> = {}): MemoEntry => ({
    id,
    kind: "text",
    title: `t-${id}`,
    createdAt: "1",
    ...partial,
  });
  const rows: MemoEntry[] = [
    entry("a", { content: "alpha notes", tags: ["work", "idea"], pinned: true }),
    entry("b", { kind: "link", title: "Docs", content: "https://docs.example.com/x/y" }),
    entry("c", { kind: "image", path: "C:\\m\\shot.png", tags: ["work"] }),
    entry("d", { content: "misc/with/slashes", tags: ["idea", "work"] }),
  ];

  // filterMemoEntries: kind + query + tag union
  const byTag = filterMemoEntries(rows, { kind: "all", query: "", activeTags: ["work"] });
  check(
    "标签筛选 = 并集",
    byTag.map((e) => e.id).join(",") === "a,c,d",
    JSON.stringify(byTag.map((e) => e.id)),
  );
  const byQuery = filterMemoEntries(rows, { kind: "all", query: "docs.example", activeTags: [] });
  check("query 命中 content", byQuery.length === 1 && byQuery[0]!.id === "b");
  const byTagQuery = filterMemoEntries(rows, { kind: "text", query: "alpha", activeTags: ["work"] });
  check(
    "kind × query × 标签叠加",
    byTagQuery.length === 1 && byTagQuery[0]!.id === "a",
    JSON.stringify(byTagQuery.map((e) => e.id)),
  );
  const bySlash = filterMemoEntries(rows, { kind: "all", query: "misc/with", activeTags: [] });
  check("正文里的斜杠只当文本匹配", bySlash.length === 1 && bySlash[0]!.id === "d");

  // splitPinnedGroup
  const split = splitPinnedGroup(rows);
  check("置顶条目浮到独立组", split.pinned.length === 1 && split.pinned[0]!.id === "a");
  check(
    "其余照常按日分组",
    split.groups.flatMap((g) => g.entries).map((e) => e.id).join(",") === "b,c,d",
    JSON.stringify(split.groups.flatMap((g) => g.entries.map((e) => e.id))),
  );

  // memoTagVocabulary
  const vocab = memoTagVocabulary(rows);
  check(
    "标签词表按频次排序",
    vocab.map((v) => `${v.tag}:${v.count}`).join(",") === "work:3,idea:2",
    JSON.stringify(vocab),
  );
  check("无标签时词表为空", memoTagVocabulary([entry("x")]).length === 0);

  // 拖拽载荷契约：所有卡可拖，自定义 type 携带 {id, kind} JSON
  const here = dirname(fileURLToPath(import.meta.url));
  const box = readFileSync(join(here, "../src/components/MemoBox.tsx"), "utf8");
  const composer = readFileSync(join(here, "../src/components/Composer.tsx"), "utf8");
  check(
    "MemoBox dragstart 用 encodeMemoDrag + memoDragText，不再只放行 file/image",
    box.includes("event.dataTransfer.setData(MEMO_DRAG_TYPE, encodeMemoDrag(entry))") &&
      box.includes('event.dataTransfer.setData("text/plain", memoDragText(entry))') &&
      !box.includes('entry.kind === "file" || entry.kind === "image") && Boolean(entry.path)'),
  );
  const dropGuard = composer.indexOf("decodeMemoDrag(event.dataTransfer.getData(MEMO_DRAG_TYPE))");
  const dropPaths = composer.indexOf("const paths = pathsFromDataTransfer(event.dataTransfer)");
  check(
    "Composer 先按自定义 type 分流 memo 拖拽，再走路径启发式",
    dropGuard > 0 && dropPaths > dropGuard,
    `decode at ${dropGuard}, paths at ${dropPaths}`,
  );
  check(
    "text/link memo 落到光标插入，file/image 走附件路径",
    composer.includes('if (memo.kind === "text" || memo.kind === "link")') &&
      composer.includes("if (plain) insertDroppedText(plain);") &&
      composer.includes("insertDroppedPaths([plain])"),
  );
  check(
    "encode/decode 往返",
    JSON.stringify(decodeMemoDrag(encodeMemoDrag(rows[1]!))) === JSON.stringify({ id: "b", kind: "link" }),
  );
  check(
    "decode 拒绝垃圾载荷",
    decodeMemoDrag(null) === null && decodeMemoDrag("not json") === null && decodeMemoDrag('{"id":"x","kind":"shell"}') === null,
  );
  check("file 卡拖拽文本是路径", memoDragText(rows[2]!) === "C:\\m\\shot.png");
  check("text 卡拖拽文本是正文", memoDragText(rows[3]!) === "misc/with/slashes");

  // 输入框存盒：按钮 + hub 行 + 副本语义
  check(
    "Composer 有存入收纳盒按钮与 hub 行",
    composer.includes('t("composer.saveToMemoTitle")') &&
      composer.includes('t("composer.saveToMemo")') &&
      composer.includes("saveDraftToMemo"),
  );
  check(
    "MemoBox 接 revision 重拉 + 标签/置顶/编辑 UI 存在",
    box.includes("revision = 0") &&
      box.includes("memo-box__group--pinned") &&
      box.includes("memo-box__tag-editor") &&
      box.includes("memo-box__edit"),
  );
}

if (failures > 0) {
  console.error(`\nverify-memo-box: ${failures} failure(s)`);
  process.exit(1);
}
console.log("\nverify-memo-box: all checks passed");
