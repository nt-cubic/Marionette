# Marionette 功能落地规范

> 日期：2026-07-29（v2，替换 07-28 的初稿）
> 状态：**可施工**
> 前置：体积优化 / PTY 移除单独推进（不在本文范围）
> 相关：`docs/06-context-bridge-and-subagents.md`（子会话与 MCP 桥接的既有决策，本文不重复决策，只引用）

---

## 阅读方式

每个功能按同一套骨架写，缺任何一项都不算规范：

| 段 | 意思 |
|---|---|
| **落点** | 改哪个已有文件 / 新建哪个文件。不允许出现"新建一个系统" |
| **契约** | 类型定义、Tauri command 签名、新增事件分支 |
| **规则** | 边界条件、冲突处理、失败态 |
| **步骤** | 可独立提交的台阶，每台阶带验收 |
| **不做** | 明确排除，防止施工时膨胀 |

标注约定：✅ = 有代码/日志证据；⚠️ = 需要先验证再动手。

---

## 目录

0. [共用底座](#0-共用底座)
1. [智能建议芯片](#1-智能建议芯片)
2. [Plan / Todo 面板](#2-plan--todo-面板)
3. [@ 派任务](#3--派任务)
4. [Provider 动态列表](#4-provider-动态列表)
5. [批注系统](#5-批注系统)
6. [实施顺序与验收总表](#6-实施顺序与验收总表)
7. [明确不做](#7-明确不做)

---

## 0. 共用底座

三个功能（Todo、@派单、批注）都要动同一批地方，先把公共约定定死。

### 0.1 SessionEvent 扩展与向下兼容

新增分支时的硬要求：

1. `SessionEvent` 是**落盘格式**（`.marionette/transcripts/{id}.jsonl`）。旧 transcript 没有新 type，新 transcript 会被旧版本读到。
2. **渲染层必须对未知 `type` 静默降级**，不能让一条陌生事件把整个会话渲染打断。当前 `SessionView` 是一串三元表达式（`SessionView.tsx:526–607`），未知 type 会落到兜底分支——**可用但会渲染成一张无意义的卡**。新增 type 时必须补显式分支。
3. 新字段一律 optional，不改已有字段语义。
4. `persistableEventsForSession` 决定哪些事件落盘——新增 type 要显式加进去，否则重启就没了。

本文一共新增 3 个分支，集中在这里，实现时一次加完：

```ts
// src/lib/types.ts —— 追加到 SessionEvent 联合体
| {
    type: "subtask_started";
    sessionId: string;          // 父会话
    childSessionId: string;
    agentId: string;
    agentLabel: string;
    modelId?: string;
    prompt: string;             // 派给子 agent 的任务文本（不含 @前缀）
    createdAt: string;
  }
| {
    type: "subtask_result";
    sessionId: string;
    childSessionId: string;
    agentId: string;
    status: "done" | "failed" | "cancelled" | "timeout";
    /** 子会话最后一条 assistant_message，截断 2000 字；全文在子 transcript */
    summary: string;
    durationMs?: number;
    error?: string;
    createdAt: string;
  }
```

（批注不新增事件——见 §5，它渲染进 `user_message.text`。）

### 0.2 存储路径约定

项目级数据一律在 `{project}/.marionette/` 下，与 `sessions/`、`transcripts/`、`handoff/`、`context.json` 平级（`app_paths.rs`）。

| 新增文件 | 内容 | 进 git |
|---|---|---|
| `{project}/.marionette/todos.json` | 项目 Todo 清单 | 是（与 `context.json` 同策略） |
| `~/.marionette/providers.json` | Provider 元信息**用户增量** | — （全局，不进任何仓库） |

**不要**把项目数据放 `~/.marionette/projects/{projectId}/`——全局目录只放跨项目的东西（`projects.json`、`providers.json`、`logs/`）。

### 0.3 复用既有机制，不要新造

| 要做的事 | 用现成的 |
|---|---|
| 往 Composer 塞一段文本让用户确认后发送 | `Composer` 的 `prefillText` + `prefillToken`（"applied once per token; does not auto-send"，`Composer.tsx:59`） |
| 把选中内容 + 评论拼成 prompt | `formatPinsForSend()`（`src/lib/quoteComment.ts`） |
| 输入框里的 token 补全菜单 | `slashQueryAtCursor` / `filterSlashCommands` / `applySlashCommand`（`src/lib/slashCommands.ts`）——`@` 补全照抄这三个函数的形状 |
| 离线拿到某个 agent 的模型列表 | `cachedCapabilitiesFor(agentId)`（`src/lib/capabilityCache.ts`），不需要连上 agent |
| 解析一种新的 ACP session/update | `parseAvailableCommandsUpdate()`（`slashCommands.ts:71`）是模板：容错取字段、返回 `null` 表示"不是这种更新" |

---

## 1. 智能建议芯片

原稿叫"智能回复"，范围太窄。真实需求是：**在用户下一步意图明确的任何时刻，把那一步变成一次点击。** 触发源不止"AI 回复完了"。

### 1.1 落点

| 文件 | 动作 |
|---|---|
| `src/lib/suggestions.ts` | 新建。纯函数，无 React、无 Tauri，可单测 |
| `src/components/Composer.tsx` | 在 `.composer__field` 上方渲染芯片行 |

**零 Rust 改动。**

### 1.2 契约

```ts
export type SuggestionSourceId =
  | "drop"      // 刚拖进来文件
  | "options"   // AI 给了编号选项
  | "question"  // AI 在问你要不要 X
  | "error"     // 上一轮失败
  | "edited"    // 上一轮改了文件
  | "idle";     // 兜底

export type Suggestion = {
  id: string;
  /** 芯片上显示的字，≤ 12 个汉字 */
  label: string;
  /** 点击后真正发出去的文本，可以比 label 完整 */
  text: string;
  source: SuggestionSourceId;
};

export type SuggestionInput = {
  events: SessionEvent[];        // 当前会话的事件（只用尾部）
  sessionStatus: SessionStatus;
  /** 本次拖拽刚插入的路径；未拖拽为空数组 */
  droppedPaths: string[];
  /** 拖拽刚插入后的草稿快照，用于判定"草稿等于没写" */
  draft: string;
  draftAfterDrop: string | null;
};

/** 最多返回 3 条；不该出现时返回空数组 */
export function suggestFor(input: SuggestionInput): Suggestion[];
```

### 1.3 优先级与显示规则

来源按优先级取**第一个非空**，不混合：

```
drop > options > question > error > edited > idle
```

`drop` 排第一：用户刚拖完文件，意图比上一条回复更近。

显示条件（全部满足）：

| 条件 | 说明 |
|---|---|
| `sessionStatus !== "running"` | 生成中不打扰 |
| 草稿为空 | **例外**：`draftAfterDrop != null && draft === draftAfterDrop` 时视为空——拖拽会往草稿插路径，不做这个例外 drop 芯片永远出不来 |
| 非 IME 组字中 | 复用 `composingRef` |
| 功能开关为 on | `localStorage["marionette-suggestions"]`，默认 on |

其它规则：

- 点击 = **直接发送**。这个功能的全部价值就是省掉输入，填草稿等于没省。
- 用户一开始打字，芯片立即消失，且**本轮不再回来**（每个 `(sessionId, 事件序号)` 只展示一次）。
- 芯片区高度固定，出现/消失用淡入，**不能挤动 Composer**——抖动比没有更烦。

### 1.4 `question` 源：从 AI 的提问里抠出动作

这是最值钱的一条，规则要写死。

**取材**：最后一条 `assistant_message` 的**末尾 200 字**（长回复中间的疑问句不算数）。先剥 markdown：去掉 `**`、`` ` ``、`#`、列表符号。

**定位**：按 `。！？!?\n` 切句，从后往前找第一个满足任一条件的句子：
- 以 `？` / `?` 结尾
- 含 `吗` / `呢` / `要不要` / `是否` / `需要我` / `用不用` / `还是`
- 英文：`should I` / `do you want` / `would you like` / `shall I` / `want me to`

**抽取 + 生成**：

| 句型 | 抽出 | 芯片 |
|---|---|---|
| `要不要 X` | X | 「<X>」·「不用」 |
| `需要我 X 吗` | X | 「<X>」·「先不用」 |
| `是否(需要) X` | X | 「<X>」·「不用」 |
| `我可以 X，要吗/好吗` | X | 「<X>」·「不用」 |
| `A 还是 B` | A, B | 「A」·「B」 |
| `Should I X?` / `Want me to X?` | X | 「<X>」·「Not now」 |
| 命中疑问但没匹配句型 | — | 「是」·「不是」·「让我想想」 |

**人称翻转（必须做）**：抽出来的 X 常带主语——`要不要我帮你重构一下` → X = `我帮你重构一下`，芯片写成「要，我帮你重构一下」读起来是错的。

```ts
/** "我帮你重构一下" → "重构一下"；"I can run the tests" → "run the tests" */
export function stripFirstPerson(action: string): string;
```

规则：去掉开头的 `我来` / `我先` / `我可以` / `我帮你` / `帮你` / `让我` / `我`；英文去掉 `I can ` / `I'll ` / `me to `。再去掉结尾的 `吗` / `呢` / `？` / `?` / `。`。

产出：
- `label` = 处理后的动作，超 12 字截断加 `…`
- `text` = `好，<动作>`（肯定项）/ `不用` `先不用`（否定项）

**例**：
```
AI: "…我已经改完了。要不要我顺便把单测也补上？"
   → label: 「顺便把单测也补上」  text: 「好，顺便把单测也补上」
   → label: 「不用」            text: 「不用」
```

### 1.5 `options` 源：编号选项直接变芯片

疑问句**紧跟着**有序/无序列表（`1. ` / `- ` / `* `）时，每项一个芯片：

- 最多 3 项，每项原文 ≤ 40 字才纳入（长条目说明这不是选项，是正文）
- `label` = 项目文本前 12 字
- `text` = `选 <n>：<项目全文>`

### 1.6 `drop` 源：拖文件即建议

**触发点**：`insertDroppedPaths()`（`Composer.tsx:1004`）执行后。按扩展名 + 数量出芯片：

| 类型 | 判定 | 芯片（取前 3） |
|---|---|---|
| 文档 | `.md .txt .rst .adoc` | 帮我看看这个文档 · 审查一下这份文档 · 总结要点 |
| 代码 | `.ts .tsx .js .jsx .rs .py .go .java .c .cpp .cs .swift .kt .rb .php` | 帮我看看这个文件 · review 一下这段代码 · 解释一下它在干嘛 |
| 图片 | `.png .jpg .jpeg .gif .webp .svg` | 看看这张图 · 按这个图实现 · 找出图里的问题 |
| 日志 | `.log`，或文件名含 `log` / `error` / `crash` | 分析这个日志 · 找出报错原因 |
| 配置/数据 | `.json .jsonc .yaml .yml .toml .ini .csv` | 看看这个配置 · 检查有没有问题 |
| 目录 | 无扩展名且路径以分隔符结尾 ⚠️ 需确认 Tauri drop 是否给目录 | 看看这个目录 · 梳理一下结构 |
| 多个文件 | `paths.length >= 2` | 帮我看看这几个文件 · 对比一下 · +（主类型第一条） |
| 未知 | 兜底 | 帮我看看这个文件 |

`text` 就是 `label` 原文——这些句子本身已经是完整意图，路径已经在草稿里了。

### 1.7 `error` / `edited` / `idle`

| 源 | 判定 | 芯片 |
|---|---|---|
| `error` | 最后一条 `assistant_message` 由 `formatClassifiedError` 产出，或本轮 `sessionStatus` 到过 `error` | 重试 · 换个思路 · 看看日志 |
| `edited` | 本轮出现过 `file_change`，或 `tool_call.toolName` ∈ {edit, write, patch} | 跑一下测试 · 看看效果 · 继续 |
| `idle` | 兜底 | 继续 · 好 · 再说说 |

### 1.8 AI 版：**先不做**，且原稿的成本估算是错的

初稿写"每次多一轮 ~50 tokens 输入 + ~10 tokens 输出，可以忽略"。在 ACP 里不成立：

- `send_prompt` 走的是**完整会话上下文**（`acp.rs:846`，`prompt: [{type:"text", text}]` 打进同一个 `agent_session_id`）。长会话追加一轮 = 几万 token 的重读。
- 这一轮会**留在 agent 的历史里**，下一轮它会看到"请生成 3 个回复建议"这条指令，行为会漂。

要做 AI 版只有一条干净的路：**绕开 ACP 会话，直接调 provider API，只带最后一条 assistant 消息**。那需要 Provider 侧的通用 chat 调用能力（现在只有余额探测）。列为远期，不进本轮。

### 1.9 步骤与验收

| 步 | 内容 | 验收 |
|---|---|---|
| 1 | `suggestions.ts` + 单测（question / options / stripFirstPerson 三组用例） | `npx tsc --noEmit` 绿；用例覆盖上表每个句型 |
| 2 | Composer 渲染 + 显示/隐藏规则 | 让 agent 问一句"要不要我…"，芯片出现且文字是抠出来的动作，不是固定词 |
| 3 | drop 源接线 | 拖一个 `.md` 进去，出现「帮我看看这个文档」，点一下直接发出，路径在消息里 |
| 4 | 开关 | localStorage 置 off 后完全不出现 |

### 1.10 不做

- AI 生成建议（见 §1.8）
- 粘贴触发（剪贴板内容判定噪音大）
- 芯片的多语言自动切换（中英关键词表同时生效即可，不做 locale 检测）

---

## 2. Plan / Todo 面板

### 2.1 先讲一个被浪费的事实 ✅

**ACP 已经在推任务清单，Marionette 全丢了。**

本机 `~/.marionette/logs/dev.log` 统计：

```
2450 agent_thought_chunk · 1175 tool_call_update · 1123 agent_message_chunk
 282 usage_update ·  262 tool_call ·   30 plan   ← 30 条，从来没被解析过
```

实际报文：

```json
{"sessionUpdate":"plan","entries":[
  {"content":"4-1 配色主题：BattleModeBPalette 单一色板","status":"pending","priority":"medium"},
  {"content":"4-2 字体三套 + 去掉全场景扫描 hack","status":"pending","priority":"medium"}]}
```

`acp.rs` 和 `acpTranscript.ts` 里 `plan` 一次都没出现。这条线接上，面板的一半直接白拿，而且**不花 token**。

### 2.2 关键区分：Plan ≠ Todo

初稿把两者混成一张列表。它们生命周期完全不同，混在一起 **agent 下一轮 plan 全量重发会把用户手写的条目冲掉**。

| | **Plan** | **Todo** |
|---|---|---|
| 归属 | session | project |
| 来源 | ACP `plan` update | 用户手输 / 从 Plan 吸收 / AI 更新 |
| 可编辑 | 只读 | 可增删改 |
| 更新方式 | **全量替换**（agent 每轮重发整张表） | 增量 |
| 落盘 | 否（跟 `liveEvents` 走，重启即消失——诚实：它是会话内状态） | `{project}/.marionette/todos.json` |
| UI | 面板上半区，标「来自 <agent> · 本会话」 | 面板下半区 |

### 2.3 落点

| 文件 | 动作 |
|---|---|
| `src/lib/acpPlan.ts` | 新建。`parseAcpPlanUpdate()`，照抄 `parseAvailableCommandsUpdate` 的容错写法 |
| `src/app/App.tsx` | acp-event handler 里加一路：`setPlanBySessionId`（紧挨着现有的 `setSlashCommandsById`，`App.tsx:416`） |
| `src/lib/todos.ts` | 新建。合并 / 匹配 / 渲染成 prompt 的纯函数 |
| `src/components/ContextPanel.tsx` | 加**第 5 张 `context-card`**（见下） |
| `src-tauri/src/commands.rs` | 两个命令 |

**关于 Tab**：初稿写"复用 Information 面板的 Tab 切换机制"——**这个机制不存在**。`ContextPanel` 是 4 张堆叠的 `<section class="context-card">`（Usage / Changed Files / Project context / Handoff）。加 Tab 要改 4 张卡 + 样式，是独立的 UX 任务。本功能加**第 5 张卡**，与现状一致。

### 2.4 契约

```ts
// src/lib/acpPlan.ts
export type PlanEntry = {
  content: string;
  status: "pending" | "in_progress" | "completed";
  priority?: "high" | "medium" | "low";
};
/** 不是 plan 更新时返回 null（与 parseAvailableCommandsUpdate 同语义） */
export function parseAcpPlanUpdate(data: unknown): PlanEntry[] | null;
```

```ts
// src/lib/types.ts
export type TodoItem = {
  id: string;                    // "todo-<base36>-<rand>"
  text: string;
  status: "todo" | "doing" | "done";
  source: "user" | "plan" | "ai";
  /** source=plan 时记录来自哪个会话，UI 上显示"来自 Claude 那轮" */
  originSessionId?: string;
  createdAt: string;
  updatedAt: string;
  doneAt?: string;
};

export type TodoFile = { version: 1; items: TodoItem[]; updatedAt: string };
```

```rust
// commands.rs —— 全量读写，清单只有几十条，不做增删改三件套
list_todos(project_id: String) -> Result<Vec<TodoItem>, String>
save_todos(project_id: String, items: Vec<TodoItem>) -> Result<(), String>
```

前端持有真相，每次改动整表写回。写盘用 `write_auth_atomic` 同款的**先写临时文件再 rename**，避免半截文件。

### 2.5 面板 UI

```
┌─ Tasks ─────────────────────────────── [↻] ─┐
│ Plan · Claude Code · 本会话                  │   ← 只读，无 plan 时整块不渲染
│   ◐ 4-1 配色主题：单一色板                    │
│   ○ 4-2 字体三套                             │
│   [↓ 吸收进 Todo]                            │
│ ─────────────────────────────────────────── │
│ Todo · 项目                                  │
│   ☐ 审查 auth.ts 的性能瓶颈                  │
│   ☑ 重构 login 页面                    ✕     │
│   [+ 添加任务]                               │
│ ─────────────────────────────────────────── │
│ [发给 AI]            [让 AI 更新…]           │
└──────────────────────────────────────────────┘
```

### 2.6 三个动作的语义（这是本节的核心）

用户要求："不仅要给用户看，还得能发回给 AI，AI 可以被我按刷新去更新。" 拆成三个明确动作，**成本不同的不要放同一个按钮**：

#### ① 吸收进 Todo — 免费

把当前会话的 Plan 条目并进项目 Todo。状态映射 `pending→todo`、`in_progress→doing`、`completed→done`，`source="plan"`，`originSessionId` = 当前会话。

去重用 §2.7 的匹配键。已存在的只更新状态，不改文本（用户可能改过措辞）。

#### ② 发给 AI — 免费（不自动发）

把 Todo 渲染成文本，**塞进 Composer 草稿**（走 `prefillText` + `prefillToken`，和 Handoff 一样"never auto-send"）。用户可以在后面补一句"先做第 2 条"再发。

```
当前项目任务清单：
1. [ ] 审查 auth.ts 的性能瓶颈
2. [~] 补充单元测试
3. [x] 重构 login 页面
```

#### ③ 让 AI 更新 — 花一轮，带确认

发一条 prompt，要求 agent 给出更新后的清单。**回收优先走 plan，零解析**：

```
这是当前项目的任务清单：
<清单文本>

请结合我们刚才的进展更新它：标记已完成的、补上新发现的。
如果你有任务清单工具（TodoWrite / plan），请直接用它输出更新后的完整清单。
否则请只回一个代码块：

```marionette-todo
[{"text":"…","status":"todo|doing|done"}]
```
```

- agent 用了 plan 工具 → 我们本来就在收 `plan` 事件 → 零解析拿到结果
- agent 回了 fenced 块 → 解析 ```` ```marionette-todo ```` 块；解析失败就只提示"AI 没有按格式回复"，**不猜**

**结果一律不直接写盘**，先弹变更预览：

```
AI 建议的变更：
  + 新增 3 条
  ✓ 标记完成 2 条
  ? AI 未提及 1 条（保留）
                      [应用]  [取消]
```

### 2.7 合并规则（不写死这条一定会丢数据）

AI 不会保留我们的 `id`。用**归一化文本**做匹配键：

```ts
/** 去首尾空白、去 markdown 标记、去尾部标点、转小写、取前 40 字 */
export function todoKey(text: string): string;
```

| 情况 | 处理 |
|---|---|
| key 命中已有条目 | 更新 `status` + `updatedAt`，**不改 text** |
| key 没命中 | 新增，`source="ai"` |
| 本地有、AI 没提 | **保留**，预览里列为"AI 未提及"。默认不删——静默删除 = 数据丢失 |
| AI 回的条目 > 50 条 | 拒绝并提示（防止它把整篇回复当清单塞进来） |

### 2.8 步骤与验收

| 步 | 内容 | 验收 |
|---|---|---|
| 1 | `parseAcpPlanUpdate` + App 接线 + 面板 Plan 只读区 | 让 Claude 做个多步任务，Plan 区跟着它的 TodoWrite 实时变 |
| 2 | `todos.json` 读写 + Todo 区 CRUD | 加一条、勾掉、删掉；重启 App 还在 |
| 3 | 吸收进 Todo | 点一下 Plan 进 Todo；agent 下一轮 plan 更新**不会**冲掉手写条目 |
| 4 | 发给 AI | 点一下草稿里出现清单文本，未自动发送 |
| 5 | 让 AI 更新 + 变更预览 | Claude（有 plan 工具）和 OpenCode（走 fenced 块）各跑一次；AI 漏提的条目仍在 |

### 2.9 不做

- 拖拽排序（MVP）
- 跨项目的全局 Todo 视图
- Todo 与 GitHub Issue / 外部系统同步
- `/add-todo` 由 Composer 解析——**行不通**：Composer 只看用户键入的内容，agent 的输出根本不经过它。AI 写 todo 的正路是 ① plan（本轮做）或 ② `docs/06` 的内置 MCP server 暴露 `add_todo` 工具（Stage 2 时顺手加）

---

## 3. @ 派任务

### 3.1 定位

Composer 输入 `@opencode deepseek 检查一下项目的安全性` → 开一个子会话让 OpenCode 的 DeepSeek 干这件事 → 结果以卡片回到当前对话流。主对话**不阻塞**，可以继续聊。

这一条等价于 `docs/06 §4.3` 里说的"**用户主动编排入口，应该先做**"，以及 §3 的子会话数据模型。**本节不重新决策那些已经拍板的约束（depth=1 / 并发≤2 / 成本可见），只引用并补齐施工细节。**

### 3.2 冲突面到底在哪（结论：可控）

Claude Code / OpenCode 用 `@` 引用文件。冲突只发生在"用户想打的 `@xxx` 恰好前缀匹配某个 agent id"。用五条规则把这个面收到几乎为零：

| # | 规则 |
|---|---|
| 1 | **只在行首触发**：`^\s*@`。复用 `slashQueryAtCursor` 的同一套行内定位逻辑 |
| 2 | **只有 token 前缀匹配到已安装 agent id / 别名时才弹菜单**。`@src/foo.ts` 不匹配任何 agent → 不弹菜单、不拦截、原文照发 |
| 3 | **只有菜单可见且用户按 Tab / Enter 选中候选时才消费按键**。菜单开着直接回车 = 正常发送 |
| 4 | **派单前缀不进 prompt**：解析出目标后，发给子 agent 的只有任务文本；原文完整保存在父流卡片上 |
| 5 | **转义**：行首 `@@` 表示字面量 `@` |

别名表（键入更顺）：

```
oc / opencode        → opencode
cc / claude / claude-code → claude-code
grok / grok-build    → grok-build
gpt / codex          → codex
```

### 3.3 语法

```
@<agent>[/<model>] <任务文本>      ← 规范形式，补全菜单插入的就是这个
@<agent> <model> <任务文本>        ← 宽松形式，第二个 token 能在该 agent 的缓存模型目录里唯一前缀匹配时才当模型
@<agent> <任务文本>                ← 用该 agent 的 preferredModel，没有则 agent 默认
```

模型候选来自 `cachedCapabilitiesFor(agentId)`（`capabilityCache.ts:191`）——**离线可用**，不需要先把目标 agent 连起来。缓存里没有该 agent（从没连过）时，菜单只显示 agent，不显示模型，并提示"首次使用该 agent，模型将用其默认值"。

补全菜单：

```
@opencode▌
  ┌────────────────────────────────────────┐
  │ opencode                    ✓ 已安装    │
  │   └ deepseek-v3      上次用过            │
  │   └ zai/glm-4.6                         │
  │ codex                       ✓ 已安装    │
  │ grok-build                  未安装 ⚠     │
  └────────────────────────────────────────┘
```

`已安装 / 未安装` 来自 `listAgentCommands()`（Composer 里已经在用，`Composer.tsx:1300`）。选中未安装的 agent 直接拒绝派单并给出安装入口。

### 3.4 执行语义

| 项 | 规范 |
|---|---|
| 目标 | 新建子 session：`parentSessionId` = 当前 session，`origin = "delegate"` |
| 左侧列表 | **不出现**。Rust 侧 `list_sessions` 只返回 `parent_session_id == None` 的，另给 `list_child_sessions(parentId)`——默认安全，前端不会忘记过滤 |
| 搜索 | `search_sessions` 同样跳过子会话 |
| cwd | 与父会话相同 |
| 上下文 | **默认不带父会话历史**。子任务是一句独立委托。带上下文的变体 v1 不做（见 §3.8） |
| 并发 | 同一父会话最多 2 个运行中子任务；超出排队，卡片显示"排队中" |
| 深度 | 1。子会话里的 `@` 不解析，原文照发 |
| 超时 | 600s 无任何事件 → `status = "timeout"`，进程停掉，transcript 保留 |
| 取消 | 卡片上「停止」→ `cancel_acp_session(childId)` |
| 结果 | 子会话最后一条 `assistant_message` 全文 → `subtask_result.summary`（截 2000 字） |
| 用量 | 子会话 usage 并入右侧 Usage，标注"含 N 个子任务"（`docs/06 §4.3` 已定"成本可见"） |
| 通知 | 子任务完成**不**单独闪任务栏，只在卡片上打点——否则并行两个子任务会连闪 |
| 父会话被删 | 级联删除其所有子会话记录与 transcript（子会话脱离父卡片后无任何入口，留着就是孤儿文件） |
| App 关闭时子任务在跑 | `shutdown_and_exit` 会 `stop_all()` 杀掉进程。重启后 `list_sessions_healed` 把子会话状态修成 `exited`，父流卡片显示 `status = "cancelled"` + "应用关闭时中断"。**不自动续跑** |

### 3.5 结果卡片

父流插一张卡，三态：

```
[⟳] @opencode/deepseek-v3 · 检查项目安全性        进行中 0:42   [停止]
[✓] @opencode/deepseek-v3 · 检查项目安全性        2分18秒  [展开] [引用]
[✕] @grok-build · 安全审计                失败：agent 未安装   [重试]
```

- **展开** = 内联展开子会话完整事件流（复用现有事件渲染组件）。**不做左右分屏**——分屏是 `docs/06 Stage 3` 的后半段，v1 不碰。
- **引用** = 把结果作为引用块塞进 Composer 草稿（`formatPinsForSend` 同款格式）。

**这个「引用」按钮把初稿的台阶 ③「结果聚合」免费实现了**：

```
你: @claude 设计登录页
你: @grok 检查项目安全性
    ↓ 两张卡片都完成
    点 Claude 卡片的 [引用] → 点 Grok 卡片的 [引用] → 补一句"改成 React 组件" → 发送
```

不需要汇总 prompt、不需要编排引擎。台阶 ④（自动互评）和 ⑤（自动编排）才需要，那是 `docs/06 Stage 4` 的 bridge MCP。

### 3.6 后端改动

```rust
// models.rs Session —— 两个 optional 字段，旧 sessions/index.json 照常读
#[serde(default, skip_serializing_if = "Option::is_none")]
pub parent_session_id: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub origin: Option<String>,      // "user" | "delegate"

// commands.rs
create_child_session(project_id, parent_session_id, agent_id, label) -> Session
list_child_sessions(parent_session_id) -> Vec<Session>
// list_sessions / search_sessions：过滤掉 parent_session_id.is_some()
```

子会话的 transcript 路径沿用现有规则 `.marionette/transcripts/{childId}.jsonl`（`docs/06 §6` 建议的就是这个）。

### 3.7 步骤与验收

| 步 | 内容 | 验收 |
|---|---|---|
| 1 | `parseDelegateToken` + 补全菜单（不执行，只解析） | 输入 `@op` 弹菜单；输入 `@src/foo.ts` 不弹；选中后草稿变成 `@opencode/deepseek-v3 ` |
| 2 | 子会话后端（字段 + 两个命令 + 三处过滤） | 建一个子会话，左侧列表里看不到，`.marionette/sessions/index.json` 里有 |
| 3 | 派单执行 + 卡片三态 | 派一单，主对话仍可输入；完成后卡片显示时长 |
| 4 | 展开 / 引用 / 停止 | 展开能看到子会话完整流；引用能把结果带进草稿；停止能中断 |
| 5 | 并发 / 深度 / 超时闸门 | 连派 3 单，第 3 单显示"排队中"；子会话里打 `@` 不触发 |

### 3.8 不做

- 左右分屏（`docs/06 Stage 3` 后半段）
- agent 自己发起 delegate（`docs/06 Stage 4`，要 bridge MCP + 权限闸门）
- 带父会话上下文的派单（要先想清楚注入多少、谁付这个 token）
- 自动互评 / 自动编排（台阶 ④⑤）

---

## 4. Provider 动态列表

### 4.1 问题的实际形状

`list_providers()` 枚举的是 `auth.json` 的 key ——那是**已配置**列表。前端硬编码的 8 个是**可添加目录**。两张表来源不同：**没配过的 provider 在 `auth.json` 里根本不存在**，所以目录不可能从它推出来。

结论：`providers.json` 不是"元信息补充"，**它就是目录**。

### 4.2 三处规范修正

| 初稿 | 问题 | 改成 |
|---|---|---|
| `providers.json` 放 `~/.marionette/` 作为唯一数据源 | 新装用户这个文件是空的 → 下拉框空白 | **内置默认目录编译进 Rust**，用户文件只存增量与覆盖，返回时 merge |
| `supportsBalanceProbe: true/false` | 用户填 `true` 也不会有余额——探测实现写死在 Rust 里（deepseek / openrouter / opencode-go / zen 四个） | `probeStrategy: "deepseek" \| "openrouter" \| "opencode-zen" \| "none"`，枚举由 Rust 拥有，JSON 只能选不能造 |
| 自定义 Provider 填 base URL | 与 `docs/06` 的底线冲突——**不写别人的配置文件**。OpenCode 用自定义端点必须在 `opencode.jsonc` 加 `provider` 块，光写 `auth.json` 无效 | **v1 砍掉**。自定义端点单独立项 |

还有一条文案要求：**这个对话框只影响 OpenCode**（`Composer.tsx:109` 的 `OPENCODE_AUTH_AGENTS` 只有 `opencode`）。Codex / Claude / Grok 各管各的凭证。标题要写清楚"OpenCode 服务商 Key"，否则用户以为给 Claude 加了 Key。

### 4.3 契约

```ts
export type ProviderInfo = {
  provider: string;
  label: string;
  hasKey: boolean;
  authKind: "api" | "oauth" | "unknown";
  /** 新增：auth.json 里有没有这一项 */
  configured: boolean;
  /** 新增：这条记录从哪来 */
  source: "builtin" | "user" | "auth";
  /** 新增：余额能不能查，前端据此显示"不支持"而不是留白 */
  probeStrategy: "deepseek" | "openrouter" | "opencode-zen" | "none";
};
```

```rust
list_providers() -> Vec<ProviderInfo>   // 内置目录 ∪ 用户目录 ∪ auth.json 实际 key
upsert_provider_meta(id, label, key_aliases: Vec<String>, probe_strategy: String) -> ()
delete_provider_meta(id) -> ()          // 只删元信息，不动 auth.json
```

`~/.marionette/providers.json`（只存增量）：

```json
{
  "version": 1,
  "providers": {
    "moonshot": { "label": "Moonshot", "keyAliases": ["moonshot", "kimi"], "probeStrategy": "none" }
  }
}
```

`provider_label()` / `provider_display_name()` / `auth_key_for()`（`provider_usage.rs:75/141/566`）三个 match 全部改成查这张合并后的表。**内置默认表就是把现在这三个 match 的内容搬进一个 `const` 数组**，行为不变，只是变成数据。

### 4.4 步骤与验收

| 步 | 内容 | 验收 |
|---|---|---|
| 1 | 内置目录常量 + 三个 match 改查表 | 现有 provider 行为完全不变（余额、标签、别名） |
| 2 | `providers.json` 读写 + merge | 手写一条 `moonshot` 进去，重启后下拉框有它 |
| 3 | `ProviderInfo` 扩字段 + 前端下拉框改数据驱动 | 手动往 `auth.json` 塞一个 `foobar`，对话框显示"foobar（已配置 · 余额不支持）"，无需改代码 |
| 4 | 添加自定义 Provider 表单（只填 id / 名称 / 别名 / Key） | 加一个能用；余额区诚实写"不支持" |

### 4.5 不做

- 自定义 base URL / 自定义端点（要写 `opencode.jsonc`，破底线，单独立项）
- 从 models.dev 之类的在线目录拉 provider 列表
- 给非 OpenCode agent 管 Key

---

## 5. 批注系统

### 5.1 先纠三个前提

**① 一半已经有了。** `src/lib/quoteComment.ts` 的 `QuotePin { quoted, comment, x, y }` + `formatPinsForSend()` 就是文本批注，App 里 `quotePins` 全链路已通（`App.tsx:2186`）。**不要新建 `Annotation` 体系，让 `QuotePin` 成为它的一个变体。**

**② 结构化 JSON 现在发不出去。** `send_prompt` 只吃字符串，Rust 侧写死：

```rust
"prompt": [{ "type": "text", "text": text }]     // acp.rs:846
```

初稿写的 `{ type: "image", src, annotations: [...] }` 要落地，得先把这里改成能构造 ContentBlock 数组。**v1 全部渲染成文本**——`formatPinsForSend` 就是既有先例。

**③ "不生成标注图"这个结论对，但前提没写。** 发坐标确实比发渲染图精确——**前提是模型能看到原图**。现在 Marionette 连图片都发不出去（拖拽只是把路径插成文本）。给一个没看过图的模型发 `x:120, y:300` 等于噪音。所以图片批注的完整前提是：

- 原图作为 image ContentBlock 一起发（→ 依赖 ② 的改造）
- 坐标用**归一化 0–1**，并附 `naturalWidth/Height`——像素坐标在模型侧无从换算

这就是为什么图片批注排最后。

### 5.2 统一数据模型

```ts
export type Annotation = {
  id: string;
  comment: string;
  createdAt: string;
} & (
  | { kind: "quote";  quoted: string; x: number; y: number }              // 现有 QuotePin
  | { kind: "range";  filePath: string; startOffset: number; endOffset: number; quoted: string }
  | { kind: "line";   filePath: string; side: "old" | "new"; lineNumber: number; quoted: string }
  | { kind: "point";  imagePath: string; nx: number; ny: number }         // 归一化 0–1
  | { kind: "rect";   imagePath: string; nx: number; ny: number; nw: number; nh: number }
);
```

**`side` 是初稿漏掉的**：`{ type:"line", filePath, lineNumber }` 在 diff 里是歧义的——旧文件的第 42 行和新文件的第 42 行是两回事。

**存储：不落盘。** 批注是"我正要问的这件事"的一部分，发送后随 `user_message.text` 进 transcript 就够了。不要建 `annotations.json`。

### 5.3 发送格式（v1，纯文本）

扩展 `formatPinsForSend` 为 `formatAnnotationsForSend`：

```
1. src/lib/auth.ts:42（新）
> const token = readToken();
评论：这里没有处理过期

2. docs/design.md
> 我们采用双写方案
评论：双写的回滚路径没写

（Composer 自由文本放最后）
```

### 5.4 三个载体的实现顺序（与初稿相反）

按依赖排，不按"哪个看起来酷"排：

| 序 | 载体 | 依赖 | 量 |
|---|---|---|---|
| 1 | **文本 / MD 选区** | 无——扩 `QuotePin` 加 `filePath` + offset | 1 天 |
| 2 | **Diff 行** | 要解析 `@@ -a,b +c,d @@` hunk 头 | 1.5 天 |
| 3 | **图片** | ContentBlock 改造 + 图片查看器（应用里现在完全没有） | 3–4 天 |

**Diff 行的额外工作**：现在 diff 就是一个逐行染色的 `<pre>`（`SessionView.tsx:309`），没有行号概念。要先写：

```ts
/** 把 unified diff 文本解析成带双侧行号的行数组 */
export function parseUnifiedDiff(text: string): Array<{
  raw: string;
  type: "add" | "del" | "ctx" | "hunk" | "meta";
  oldLine: number | null;
  newLine: number | null;
}>;
```

有了它顺带把 diff 卡片升级成带行号的样子——本来也该有。

### 5.5 步骤与验收

| 步 | 内容 | 验收 |
|---|---|---|
| 1 | `Annotation` 类型 + `formatAnnotationsForSend`，现有 quotePins 迁过去 | 现有引用评论功能行为不变 |
| 2 | MD / 文本选区批注 | 在 Clean 里选中一段 markdown 加评论，发送后 prompt 里带文件名和引文 |
| 3 | `parseUnifiedDiff` + diff 卡片显示行号 | 行号与 `git diff` 对得上（新增/删除/上下文三种行各验一处） |
| 4 | Diff 行批注 | 点行号加评论，发送后带 `path:line（新）` |
| 5 | （另立项）ContentBlock 改造 → 图片查看器 → 图片批注 | — |

### 5.6 不做

- 生成标注过的图片（结论不变：坐标 + 原图更精确）
- 批注落盘 / 跨会话保留
- 富文本批注（只有纯文本 comment）

---

## 6. 实施顺序与验收总表

| 序 | 功能 | 量 | 依赖 | 一句话验收 |
|---|---|---|---|---|
| 1 | **接 ACP `plan`** | 0.5 天 | 无 | 让 Claude 做多步任务，右侧 Plan 区实时跟着变 |
| 2 | **智能建议芯片** | 1 天 | 无 | 拖一个 md 进去出现「帮我看看这个文档」；AI 问"要不要 X"出现「X」 |
| 3 | **Todo 面板 + 三个动作** | 1.5 天 | 1 | 让 AI 更新清单，变更预览里 AI 漏提的条目仍在 |
| 4 | **Provider 数据驱动** | 1.5 天 | 无 | 手动往 auth.json 塞 `foobar`，对话框认得它 |
| 5 | **@ 派任务** | 4–5 天 | 无（但最好在 1–3 之后） | 派两单并行，主对话不阻塞；「引用」能把两份结果带进草稿 |
| 6 | **批注：文本 → Diff** | 2.5 天 | 无 | 点 diff 行号加评论，prompt 里带 `path:line（新）` |
| — | 图片批注 / 分屏 / bridge MCP | 远期 | ContentBlock 改造、`docs/06 Stage 3-4` | — |

排序理由：

- **1 排第一**：协议白给、零 token、半天，而且直接把 3 做掉一半。
- **2 排第二**：零后端改动，改完立刻有感知，能验证"用户到底要不要这个"。
- **5 排在 1–3 之后**：它是唯一要动 Rust 会话模型的，前面几件把面板和输入区的手感先打磨好，派单的结果卡片才有地方安放。

---

## 7. 明确不做

跨功能的统一排除项，实现时任何一条被提出来都可以直接指这里：

1. **AI 生成的智能回复**（走 ACP 追加轮次 = 全上下文重读 + 污染历史；见 §1.8）
2. **`/add-todo` 由 Composer 解析**（agent 输出不经过 Composer；见 §2.9）
3. **自定义 Provider base URL**（要写 `opencode.jsonc`，破"不写别人配置文件"的底线；见 §4.5）
4. **子会话左右分屏**（`docs/06 Stage 3` 后半段）
5. **agent 自己发起 delegate**（`docs/06 Stage 4`，要权限闸门）
6. **生成标注过的图片**（坐标 + 原图更精确）
7. **批注落盘**
8. **给 ContextPanel 加 Tab**（独立 UX 任务，本轮用第 5 张卡）

---

## 相关文档

- `docs/06-context-bridge-and-subagents.md` — 子会话 / MCP 桥接的既有决策（§3 laneId、§4 delegate 与硬上限）
- `docs/05-next-roadmap.md` — 已完成项与产品底线
- `docs/07-session-aggregation-and-diff.md` — 会话聚合与 diff
