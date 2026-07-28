# 会话聚合 + 刷新按钮 + Diff 折叠卡片

> 状态：**已实现**（Grok / Claude / Codex / OpenCode + Diff 卡片 + Marionette 全量改名）
> 产品名：Marionette（包名/二进制/数据目录 `.marionette`；启动时从 `.agentshell` 一次性迁移）
> 来源：与 Codeg-main 的对比研究 + 用户需求讨论 + 本机 Agent 存储实测
> 底线：不做启动自动扫描、不做导入/导出流程、不改变现有 UI 风格

---

## 0. 一句话总结

三件事都是**马上能用**的功能：

| # | 功能 | 一句话 |
|---|------|--------|
| 1 | **会话聚合** | 点刷新，Grok / Claude / Codex / OpenCode 属于**当前项目**的旧会话出现在列表 |
| 2 | **刷新按钮** | 会话列表加 ↻，后台并发只读扫描，不阻塞 UI |
| 3 | **Diff 折叠卡片** | `file_change` 消息可展开，展示**当前工作区**对该文件的 `git diff` |

---

## 1. 会话聚合

### 1.1 做什么

Marionette 自己的会话来自项目内 `.agentshell/transcripts/*.jsonl`。

会话聚合 = **只读扫描**其他 Agent 的会话元数据，按当前项目 `rootPath` 过滤后展示；点开时再读完整对话并映射为 Marionette 的 `SessionEvent[]`（与 `load_transcript` 同形）。

### 1.2 扫哪些 Agent（路径以 Windows 实测为准）

| Agent | 存储位置（本机实测） | 格式 | 列表扫描读什么 | 点开读什么 |
|-------|---------------------|------|----------------|------------|
| **Grok** | `%USERPROFILE%\.grok\sessions\<编码 cwd>\<uuid>\summary.json` + 同目录 `updates.jsonl` | JSON + JSONL | 只读 `summary.json`（title / cwd / times） | 解析 `updates.jsonl` → 事件 |
| **Claude Code** | `%USERPROFILE%\.claude\projects\<编码项目路径>\*.jsonl` | JSONL（**不是** `~/.claude/sessions/*.json`） | 目录枚举 + 读文件首部/meta 行取标题与时间 | 整文件解析 user/assistant 行 |
| **Codex** | `%USERPROFILE%\.codex\sessions\YYYY\MM\DD\rollout-*.jsonl`；可选索引 `%USERPROFILE%\.codex\session_index.jsonl` | JSONL（按日期子目录） | 优先 `session_index.jsonl`；否则递归 `sessions/**` 读 `session_meta` 行 | 整 JSONL → 消息事件 |
| **OpenCode** | `%USERPROFILE%\.local\share\opencode\opencode.db`（**不是** `~/.config/opencode/opencode.db`） | SQLite | 带 **cwd/project 条件** 的 `SELECT` 摘要（禁止全表扫） | 按 session id 读 message/part 拼对话 |

> **OpenCode 注意**：本机 `opencode.db` 可达数 GB。列表接口必须用索引/条件查询；超时或失败时该 parser 单独报错，不影响其他三家。

### 1.3 cwd 匹配（筛选逻辑）

只展示属于**当前项目**的外部会话。

比较前必须做路径规范化（Windows）：

1. 去掉 `\\?\` 前缀  
2. `/` → `\`  
3. 去尾部 `\`（根盘符 `D:\` 除外）  
4. 大小写不敏感比较  
5. 匹配：`session_cwd == project_root` **或** `session_cwd` 在 `project_root` 之下（子目录会话可选纳入；MVP 默认 **等值或规范化后等值**，子目录会话也纳入）

Grok 实测 `cwd` 常为 `\\?\D:\Myself\AgentsShell`，项目 root 多为 `D:\Myself\AgentsShell`——必须规范化后再比。

### 1.4 不做什么

- ❌ 不把外部会话写入 Marionette DB / 不复制进 `.agentshell/transcripts`
- ❌ 不启动自动扫描（仅用户点刷新）
- ❌ 不修改外部会话文件（只读）
- ❌ 外部会话 **不可** 当作活会话继续 ACP/PTY 聊天（只读回看）
- ❌ 外部会话不进删除/重命名/handoff 流程

### 1.5 后端：`parsers/`

```
src-tauri/src/parsers/
├── mod.rs         ← AgentParser trait + scan_all / load_one
├── path_norm.rs   ← cwd 规范化 + 项目匹配
├── grok.rs
├── claude.rs
├── codex.rs
└── opencode.rs
```

```rust
/// 列表项（前端展示用）
struct ExternalConversation {
    /// 稳定 id：`{source}:{native_id}`，如 `grok:019f91f8-...`
    id: String,
    source: String,          // "grok" | "claude" | "codex" | "opencode"
    title: String,
    cwd: String,
    started_at: Option<String>,
    last_active_at: Option<String>,
    native_id: String,
    /// 打开对话时 parser 需要的本地线索（路径或 db key），不展示
    locator: String,
}

trait AgentParser: Send + Sync {
    fn source(&self) -> &'static str;
    /// 扫描属于 project_root 的会话摘要；单 parser 失败返回 Err，调度层吞掉并记日志
    fn list(&self, project_root: &str) -> Result<Vec<ExternalConversation>, String>;
    /// 读完整对话，映射为与 load_transcript 相同的 JSON 事件数组
    fn load(&self, locator: &str) -> Result<Vec<serde_json::Value>, String>;
}
```

依赖：

- OpenCode：`rusqlite`（只读打开 `opencode.db`，`OpenFlags::SQLITE_OPEN_READ_ONLY`）
- 其余：标准库 + 已有 `serde_json`

### 1.6 Tauri 命令契约

```text
list_external_sessions(projectId: string)
  → ExternalConversation[]
  行为：解析 project.root_path；并发（或顺序）调用 4 个 parser.list；
        合并去重 by id；单源失败 → 该源空列表 + debug_log，不 fail 整命令

load_external_session(source: string, locator: string)
  → SessionEvent[]  // 与 load_transcript 同形的 JSON 数组
  行为：按 source 选 parser；只读 load；映射失败返回可读错误字符串
```

前端 `api.ts` 增加对应 `invoke` 封装。

### 1.7 前端

#### 数据

```ts
type ExternalConversation = {
  id: string;           // "grok:uuid"
  source: "grok" | "claude" | "codex" | "opencode";
  title: string;
  cwd: string;
  startedAt?: string;
  lastActiveAt?: string;
  nativeId: string;
  locator: string;
};
```

状态（按 `projectId` 缓存，**不落盘**）：

- `externalByProject: Map<projectId, ExternalConversation[]>`
- `externalScanning: boolean`
- `externalStatus: string`（如「找到 5 个外部会话」/「Grok 扫描失败」）
- `viewingExternal: { conv, events } | null` — 当前若打开的是外部只读会话

#### 列表 UI（`ProjectShelf`）

在**当前展开项目**的本地会话下方增加：

```
├─ <项目名>
│  ├─ (本地 sessions…)
│  ├─ 外部会话（N）          ← 仅刷新后有数据时显示；可折叠
│  │  ├─ [Grok] title
│  │  ├─ [Claude] title
│  │  └─ …
│  └─ ↻ 刷新外部会话         ← 项目行或分区旁
```

- 来源标签：`[Grok]` / `[Claude]` / `[Codex]` / `[OpenCode]`
- 点击：调用 `load_external_session`，Clean View 渲染事件；**禁用** Composer 发送 / Agent 切换 / Raw 启动（只读条提示）
- Search threads：MVP **不**搜外部正文（避免静默读盘）；仅本地 + 已缓存的外部标题可搜（可选，默认标题本地 filter）

#### 映射到 SessionEvent（点开）

最小集合即可渲染：

| 外部含义 | → SessionEvent.type |
|----------|---------------------|
| 用户文本 | `user_message` |
| 助手文本 | `assistant_message` |
| 思考/reasoning | `thought` |
| 工具调用 | `tool_call`（能解析多少算多少） |
| 其它 | 可跳过或降级为 `assistant_message` |

`sessionId` 使用外部稳定 id（`grok:…`），`createdAt` 有则用原时间，无则用 epoch 占位。

---

## 2. 刷新按钮

### 2.1 行为

1. 用户点击 ↻  
2. 按钮 disabled +「正在扫描…」  
3. `list_external_sessions(currentProjectId)`  
4. 成功：更新该项目缓存；文案「找到 N 个外部会话」  
5. 失败：文案错误；不清空旧缓存（若有）  
6. 按钮恢复  

### 2.2 设计要点

- 结果只在前端内存；关 App 即丢，再点再扫  
- 去重 key = `ExternalConversation.id`  
- 不阻塞输入与本地会话切换  
- 扫描范围：仅当前项目 `rootPath` 匹配  

---

## 3. Diff 折叠卡片

### 3.1 做什么

`file_change` 目前只显示 `modified: path`。在卡片上增加折叠区，展开时调用已有 `getFileDiff(projectId, path)` 展示文本 diff。

### 3.2 正确性边界（必读）

`get_file_diff` = 项目根上 **当前工作区** 的 `git diff -- path`（见 `git_service.rs`）。

| 场景 | 是否准确 |
|------|----------|
| 本会话刚改完、工作区仍是那一版 | ✅ |
| 之后又改过 / 已 commit / 文件已删 | ⚠️ 显示的是「现在」的 diff，不是历史快照 |
| 外部只读会话里的「改过某文件」 | ⚠️ 同样是当前 git，不是当时内容 |

**MVP 明确接受上述限制**；不做 blob 快照、不做历史 blame。UI 可在展开区底部小字：`当前工作区 diff，非历史快照`。

### 3.3 交互

```
┌─ file_change ─────────────────────
│  ✏️ src/index.ts · modified    ▶
└──────────────────────────────────

                ▼ 展开

┌─ file_change ─────────────────────
│  ✏️ src/index.ts · modified    ▼
│  @@ -42,6 +42,8 @@
│  +  console.log("clicked");
│  -  return false;
│  （当前工作区 diff，非历史快照）
└──────────────────────────────────
```

### 3.4 样式

保持现有黑白风格：

- `+` 行：略浅底  
- `-` 行：略深底  
- 上下文：默认文字  
- 等宽、可横向滚动、高度上限（与 `ClippedBody` 类似）

### 3.5 实现要点

1. `SessionView`（或抽出 `FileChangeCard`）：折叠状态 per `path+createdAt`  
2. 首次展开：`getFileDiff(projectId, path)`，结果缓存在组件 state  
3. 行渲染：按行首 `+` / `-` / 其它分 class  
4. 右侧 Information 的 Diff 预览可继续用现有逻辑；Clean 卡片内是就地展开，不必跳转  

---

## 4. 不做的事

| 事情 | 原因 |
|------|------|
| MCP 市场 / 技能管理 | 已有 context_inventory |
| 启动自动扫描 | 性能 |
| 手动导入对话框 | 多余 |
| 外部会话可续聊 | 协议与进程模型不同 |
| @ 委派子 Agent | 另案 |
| 更多 Agent（Gemini/Cline…） | 当前不用 |
| 历史精确 diff 快照 | 超出 MVP |

---

## 5. API / 类型 checklist

- [x] `ExternalConversation`（Rust + TS，camelCase serde）  
- [x] `list_external_sessions` / `load_external_session` 注册到 `main.rs`  
- [x] `parsers::list_all` 单源失败隔离  
- [x] Grok `summary.json` 字段：`info.cwd` / `session_summary`|`generated_title` / times  
- [x] Claude：`projects/<encoded>/*.jsonl`  
- [x] Codex：日期目录 + `session_meta` / `session_index`  
- [x] OpenCode：只读 SQLite + cwd 条件（`directory` 精确/前缀查询）  
- [x] 前端只读条 + 禁用发送  
- [x] Diff 卡片 +「非历史快照」提示  
- [x] 产品/数据目录：`.marionette`（启动时从 `.agentshell` 一次性 rename 迁移）

---

## 6. 工作顺序

| 步骤 | 内容 | 工作量 |
|------|------|--------|
| **第一步** | `parsers/` + path_norm + **GrokParser** + 两个 Tauri 命令 | 0.5–1 天 |
| **第二步** | 前端刷新 + 外部分区 + 只读打开 | 0.5 天 |
| **第三步** | ClaudeParser → CodexParser → OpenCodeParser | 1–2 天 |
| **第四步** | Diff 折叠卡片（可与第一步并行） | 0.5 天 |

验收（Grok MVP）：

1. 打开含 Grok 历史的项目 → 点刷新 → 列表出现带 `[Grok]` 的项  
2. 点开 → Clean View 看到 user/assistant（至少）  
3. Composer 不可发送；提示只读  
4. 有 `file_change` 时展开见当前 git diff  

---

## 7. 路径速查（纠偏对照）

| 文档旧写法（已废弃） | 正确写法 |
|----------------------|----------|
| Claude: `~/.claude/sessions/*.json` | `~/.claude/projects/<encoded>/*.jsonl` |
| OpenCode: `~/.config/opencode/opencode.db` | `~/.local/share/opencode/opencode.db`（Windows 实测） |
| Codex: 扁平 `sessions/<uuid>.jsonl` | `sessions/YYYY/MM/DD/rollout-*.jsonl` + 可选 `session_index.jsonl` |
| 「前端完全不用改」 | 列表 + 只读打开 + API 均需改；**事件渲染**可复用 |
| Diff = 历史变更 | Diff = **当前工作区** git |
