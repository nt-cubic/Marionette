# 各家 coding agent 的记忆存储位置 — 全网调查（2026-10-23）

背景：`src-tauri/src/memory.rs` 的跨 agent 记忆导入只扫 4 个写死路径
（`~/.claude/CLAUDE.md`、`~/.codex/AGENTS.md`、`~/.config/opencode/AGENTS.md`、`~/.gemini/GEMINI.md`），
用户在另一台机器上「刷新记忆」刷出空列表。本文把主流 agent 的**指令层**（人写的全局规则）
和**自动记忆层**（agent 自己积累的笔记）的真实落盘位置调查清楚，方法 = 官方文档 + 本机实证互相印证。

## 结论速览

| Agent | 全局指令（人写） | 自动记忆（agent 写） | 默认状态 |
|---|---|---|---|
| Claude Code | `~/.claude/CLAUDE.md` | `~/.claude/projects/<repo-slug>/memory/*.md`（MEMORY.md 索引 + 主题文件） | 自动记忆默认开 |
| Codex (GPT) | `~/.codex/AGENTS.md`（`$CODEX_HOME`） | `~/.codex/memories/`（MEMORY.md、memory_summary.md…）+ 状态库 `memories_1.sqlite` | **默认关**，`[features] memories = true` |
| Grok Build | —（无独立全局指令文件） | v1 `~/.grok/memory/MEMORY.md`；v2 `~/.grok/memory-v2/{global,workspaces/<slug>}/`：MEMORY.md 索引 + `topics/*.md` + `observations/` | 默认开 |
| Gemini CLI | `~/.gemini/GEMINI.md` | Auto Memory（实验）：候选进项目 memory 目录的 review inbox，全局只写回 `~/.gemini/GEMINI.md` | 默认关，`experimental.autoMemory: true` |
| OpenCode | `~/.config/opencode/AGENTS.md` | 无原生记忆（只有第三方插件：Supermemory/Mind/Hindsight/deja…） | — |
| ZCode (Z.ai) | `~/.zcode/AGENTS.md` | Project Memory（默认关，路径未公开，机器本地） | 记忆默认关 |
| DSH (DeepSeek) | 无全局指令文件（走 profile/项目 AGENTS.md） | 无记忆存储（本机 `~/.dsh` 只有 profiles/sessions/storages） | — |
| pi / hermes | 无全局指令/记忆文件（`~/.pi/agent/`、`~/.hermes/` 只有 auth/sessions/skills） | 无 | — |
| Windsurf | `~/.codeium/windsurf/memories/global_rules.md`（≤6000 字符） | `~/.codeium/windsurf/memories/`（本地 markdown，按 workspace） | 默认开 |
| Cursor | User Rules 存在设置里（无官方全局目录） | 项目 `.cursor/rules/*.mdc`；Memories 由产品管理 | — |
| GitHub Copilot | CLI 用户级 `~/.copilot/instructions.md`；仓库级 `.github/copilot-instructions.md` | Copilot Memory 存云端（preview；VS 侧可落到 `%USERPROFILE%/copilot-instructions.md`） | — |

> Windows 上 `~` = `%USERPROFILE%`。OpenCode 在 Windows 上同样用 `~/.config/opencode/`（XDG 风格，本机实证）。

## 各家详述

### Claude Code（官方：code.claude.com/docs/en/memory）

**指令层（CLAUDE.md 体系）：**

- 用户全局：`~/.claude/CLAUDE.md` —— Marionette 现在唯一收的 Claude 路径。
- 企业托管：Windows `C:\Program Files\ClaudeCode\CLAUDE.md`、macOS
  `/Library/Application Support/ClaudeCode/CLAUDE.md`、Linux `/etc/claude-code/CLAUDE.md`。
- 项目：`./CLAUDE.md` 或 `./.claude/CLAUDE.md`；子目录 CLAUDE.md 按需懒加载；`./CLAUDE.local.md` 个人本地。
- rules：项目 `.claude/rules/*.md`、用户 `~/.claude/rules/*.md`。
- v2.1.277+ 支持 AGENTS.md 作为项目指令的**回退**（仅当没有 CLAUDE.md 时）。

**自动记忆（Auto Memory）—— Marionette 完全没收的一层：**

- 位置：`~/.claude/projects/<project>/memory/`。`<project>` 由 **git 仓库根**推导（路径斜杠转横线），
  同仓库的所有 worktree/子目录共享同一份记忆。
- 结构：`MEMORY.md` 索引（每次会话注入，截前 200 行 / 25KB）+ 若干主题文件（如 `user_role.md`、
  `feedback_testing.md`），主题文件按需读取、不进启动上下文。
- 可用 settings 的 `autoMemoryDirectory` 覆盖位置。
- **本机实证**：`~/.claude/projects/D--Myself-Unity-Jingzhe/memory/battlemodeb-*.md` 等 5 个主题文件；
  `D--Myself-Marionette/memory/` 目录存在但空。常见误解是记忆在项目的 `.claude/` 里，实际全部在用户主目录。

### OpenAI Codex / GPT（官方：learn.chatgpt.com/docs/customization/memories，源码 openai/codex `codex-rs/memories/README.md`）

**指令层**：`$CODEX_HOME/AGENTS.md`（默认 `~/.codex/AGENTS.md`）。官方定位是「必守的团队规则」放这里。

**记忆层（Memories）—— Marionette 没收：**

- 落盘：`~/.codex/memories/`，纯 markdown：`MEMORY.md`（主记忆）、`memory_summary.md`（紧凑摘要）、
  `raw_memories.md`、`rollout_summaries/`、`skills/`；目录内有 `.git` 基线做增量 diff。
- 管线状态另存 `~/.codex/memories_1.sqlite`（stage1 逐会话提取 + phase2 全局合并的调度/去重状态，
  `stage1_outputs` 表）——**sqlite 只是内部状态，用户可见记忆都是 markdown**，不必解析它。
- **默认关闭**。开启：`config.toml` 里 `[features] memories = true`；细粒度键：
  `memories.generate_memories`（新会话能否成为记忆来源）、`memories.use_memories`（注入后续会话）、
  `memories.disable_on_external_context`、`memories.extract_model` / `consolidation_model`、
  `memories.min_rate_limit_remaining_percent`。会话内 `/memories` 可临时开关。
- 生成时机：会话 **idle 一段时间后**后台跑（不是会话结束即写），接近限流阈值时跳过。
- **本机实证**：`AGENTS.md` 0 字节；`memories_1.sqlite` 存在但 `stage1_outputs` 0 行、`memories/` 目录空
  ——即功能没开/没产出。**Marionette 现在扫的 Codex 路径（AGENTS.md）在大多数机器上注定为空。**

### Grok Build / xAI（官方：x.ai/news/grok-build-memory，2026-09-16）

两代布局并存：

- **v1**：`~/.grok/memory/MEMORY.md` —— 单文件全局记忆，自动管理但**可手编**（下个会话重新索引）。
- **v2（现行）**：`~/.grok/memory-v2/` 下分作用域：
  - `global/` —— 全局偏好；
  - `workspaces/<slug>/` —— 每个工作区一份。
  - 每个作用域内：`MEMORY.md`（**生成索引**，标注 "Do not edit"）、`topics/*.md`（一题一文件，
    如 `preferences.md`、`testing.md`、`code-style.md`）、`observations/`（每轮结束后自动捕获的原始笔记，
    文件名形如 `<uuid>__t000006-000006__n000.md`）、`archive/`、`index.sqlite`（FTS 检索索引）、
    `memory_state.sqlite`（tombstone/隔离/审计等状态）。
- 交互：`/memory` 只读浏览器（Global / Workspace 分组）、`/dream` 把 observations 合并进 topics。
- **同样只需收 markdown，sqlite 不用碰。**
- **本机实证**：完整 v1+v2 结构都在，`No topics have been recorded yet`（还没攒下记忆）。

### Gemini CLI（官方：geminicli.com/docs/cli/gemini-md + repo docs/cli/auto-memory.md）

**指令层**：`~/.gemini/GEMINI.md`（全局）+ 项目 `GEMINI.md` / `.gemini/GEMINI.md` 层级，支持 `@import`。

**Auto Memory（实验，默认关）**：`~/.gemini/settings.json` 里 `experimental.autoMemory: true` 开启。
后台挖 `~/.gemini/tmp/<project>/chats/` 的历史会话（要求 idle ≥3 小时、≥10 条用户消息），产出候选进
「项目 memory 目录」的 review inbox，`/memory inbox` 审批：技能晋升到 `~/.gemini/skills/`（或项目
`.gemini/skills/`），全局补丁**只写 `~/.gemini/GEMINI.md`**，不能动项目 GEMINI.md。很多次运行不产生任何候选。

### ZCode（官方：zcode.z.ai/en/docs/agents）—— Marionette 正在接入的 agent

- **用户全局指令：`~/.zcode/AGENTS.md`**（个人偏好、跨项目约定）。加载顺序：全局在前、工作区 AGENTS.md
  追加在后；**不做多级目录合并**、不支持 `@import`。CLAUDE.md 只在首次 onboarding 时作为迁移来源读一次，
  运行时不读。
- **Project Memory（agent 写）**：成功回合后自动提炼事实、新会话召回；仅主会话读写（子代理不碰）、
  只存本机不进 git。**默认关**（Settings → General → Memory），且**官方未公开落盘路径**（本机
  `~/.zcode/v2/` 下只有 setting/bot-state/tasks-index.sqlite 等，没开过就没有痕迹）。要收它得等开启后
  实测定位路径——暂列为不可收。
- 桌面版装在任意目录、不上 PATH：Marionette 侧已用 `.lnk` 解析发现安装目录（见 acp.rs 的
  `zcode_bin_path()`，2026-10-23）。

### OpenCode（官方：opencode.ai/docs/rules）

- 项目 `./AGENTS.md`（随仓库提交）+ 全局 `~/.config/opencode/AGENTS.md`（个人偏好，不进 git）；
  V2 还会沿 cwd→home 收链路上的 ambient AGENTS.md。
- 无原生持久记忆，语义记忆靠第三方插件（opencode-mem、Mem0 集成等）。

### DSH（@deepseek-ai/dsh，本机实证）

`~/.dsh` 下只有 `profiles/`（acp/web）、`sessions/`（按工作区分目录的会话 JSONL）、`storages/`、
`cordis.patch.yml` 等，**没有全局指令文件也没有记忆存储**。指令只有会话工作目录里的项目 AGENTS.md 一层。

### 顺带：Windsurf / Cursor / Copilot

- **Windsurf**：`~/.codeium/windsurf/memories/` 全是本地 markdown——`global_rules.md`（≤6000 字符）+ 各
  workspace 记忆；不跨机同步（本机只有 skills/，没用过它的记忆）。
- **Cursor**：项目规则 `.cursor/rules/*.mdc` 随仓库；全局 User Rules 存在编辑器设置里，**没有官方全局
  规则目录**（`~/.cursor/rules` 只是社区 workaround）；Cursor Memories 由产品云端管理。
- **GitHub Copilot**：自定义指令三层——CLI 用户级 `~/.copilot/instructions.md`、仓库级
  `.github/copilot-instructions.md`、个人 `%USERPROFILE%/copilot-instructions.md`（VS 侧 Memories 可写入）；
  Copilot Memory 本体在云端（repo facts + user prefs，preview）。

## 对 Marionette memory 模块的落地建议

> **状态：已实现（2026-10-23，`memory.rs`）**——grok / grok-workspace（工作区级）之外全部落地；
> 空列表时面板展示各源探测结果（`memory_import_preview`）。同步改用全文比对（不依赖 mtime
> 粒度），claude-auto 条目随项目目录消失自动回收，纯标题/注释骨架的模板文件不计为命中。

**先对齐阵容**：`agent_registry.rs` 现行 ACP agent 是 codex、claude-code、grok-build、opencode、
pi、hermes、deepseek(dsh)、zcode（接入中）——**Gemini 已不在列**，`IMPORT_TARGETS` 里扫
`~/.gemini/GEMINI.md` 是死路径，应删。DSH、pi、hermes 本身没有全局记忆/指令文件，无可收。

`IMPORT_TARGETS` 建议（按现行阵容；全是纯文本，机制不变）：

| source | 路径 | 优先级/备注 |
|---|---|---|
| `claude` | `~/.claude/CLAUDE.md` | 维持 |
| `claude-auto` | `~/.claude/projects/*/memory/MEMORY.md`（无索引则收目录下全部 `*.md`） | 每项目一条，id 用 `import-claude-auto-<slug>` |
| `codex` | `~/.codex/AGENTS.md` | 维持 |
| `codex-mem` | `~/.codex/memories/MEMORY.md`（或 `memory_summary.md`） | 默认关的功能，多数机器没有；exists=false 正常 |
| `grok` | 优先 `~/.grok/memory-v2/global/MEMORY.md`，回退 `~/.grok/memory/MEMORY.md` | 索引文件只有路径列表时，可直接收 `topics/*.md` 拼接 |
| `grok-workspace` | `~/.grok/memory-v2/workspaces/*/MEMORY.md` | 可选，量可能大 |
| `opencode` | `~/.config/opencode/AGENTS.md` | 维持（opencode 无原生记忆，这就是全部） |
| `zcode` | `~/.zcode/AGENTS.md` | 新增，配合 zcode agent 接入；Project Memory 路径未公开，暂不可收 |
| ~~`gemini`~~ | ~~`~/.gemini/GEMINI.md`~~ | **删除**（agent 已下架） |

配套：把 `import_sources()` 的 exists/bytes/excerpt 在 UI 展示出来（`memory_import_preview` 已返回），
空列表时明确「扫过哪、命中没」，避免再出现「刷不出但不知道为什么」。

## 来源

- Claude：<https://code.claude.com/docs/en/memory>（auto memory 路径、scopes、AGENTS.md 回退）
- Codex：<https://learn.chatgpt.com/docs/customization/memories>、
  <https://github.com/openai/codex/blob/main/codex-rs/memories/README.md>（管线与产物）、
  <https://github.com/openai/codex/issues/19758>（topics 方向讨论）
- Grok：<https://x.ai/news/grok-build-memory>（scopes/topics/observations、/dream、/memory）
- Gemini：<https://geminicli.com/docs/cli/gemini-md>、
  <https://github.com/google-gemini/gemini-cli/blob/main/docs/cli/auto-memory.md>
- OpenCode：<https://opencode.ai/docs/rules>、<https://opencode.ai/v2/docs/migrate-v1>、
  <https://opencode.ai/docs/ecosystem>（记忆插件生态）
- ZCode：<https://zcode.z.ai/en/docs/agents>（`~/.zcode/AGENTS.md`、Project Memory 开关与边界）
- Windsurf：<https://docs.devin.ai/desktop/cascade/memories>
- Cursor：<https://cursor.com/docs/rules>、<https://forum.cursor.com/t/global-cursor-rules-directory/50049>
- Copilot：<https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-custom-instructions>、
  <https://docs.github.com/en/copilot/concepts/agents/copilot-memory>
- 本机实证：`~/.claude/projects/*/memory/*.md`、`~/.codex/memories_1.sqlite`（schema 已 dump）、
  `~/.grok/memory{,-v2}/`、`~/.dsh/`、`~/.codeium/windsurf/`
