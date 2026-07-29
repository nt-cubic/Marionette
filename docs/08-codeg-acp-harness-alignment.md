# Codeg 对齐：ACP 完整性 + Agent Harness

> 状态：**日常用 Grok/Codex/Claude/OpenCode 的协议主路径已齐**；真机仍要你点一遍  
> 日期：2026-07-30  
> 更新：2026-07-30 — Codex goal/retry meta、Claude 子代理流、preflight 菜单、自定义 agent、超时常量  

### 子代理（各 agent）结论（对过 Codeg）

| Agent | 能否看到子代理输出 | 机制 |
|-------|-------------------|------|
| **Claude Code** | **可以**（≥ claude-agent-acp 0.63） | 客户端广告 `_meta.subagent-transcript`；chunk 带 `parentToolUseId` → 我们写入父 Agent 工具卡 |
| **CodeBuddy** | 部分 | meta `isSubagent`；Codeg 会抑制主轨；我们不抑制，嵌套 transcript 若有则进工具 detail |
| **Codex** | 结构化 goal/retry 有 | `_meta.codex.goal` / `.error`；**不是**通用 subagent 文本轨 |
| **Grok / 多数** | **通常不能** | 子进程审批/输出不回传父 ACP；Marionette 的 `@` 委派是**我们自己开子 session**，与厂商原生 subagent 不同 |
| **Marionette `@` 委派** | 可以 | 自有 `subtask_*` 事件 + 子 session |

Codeg **没有**「任意 agent 的任意 subagent 都能拿到」的魔法；Claude 的转发是 **opt-in 能力位**，我们已广告并开始消费。
> 参考实现：`D:\垃圾\codeg-main`（及 `_research/repos/codeg`）  
> 相关文档：`docs/05-next-roadmap.md`、`docs/06-context-bridge-and-subagents.md`

### 白话：什么影响你「能不能用」

| 你在意的 | 现在怎样 | 缺了会怎样 |
|----------|----------|------------|
| 对话能不能正常聊、消息别丢别卡 | 主路径在；仍要冷启动验收 | 进程死了以前会卡在 Ask/Plan 卡上 → **已修：自动清掉** |
| Plan 批准 / Ask 选择题 | 协议 + 卡都有 | 回包错会 client disconnected → 已按 wire 修过 |
| 终端工具 / 读写文件 | 有实现 | 冷启动旧二进制会 -32601 |
| 同窗换 agent、项目 MCP/Skill | 产品硬约束，别拆 | 换 agent 丢历史才是大事故 |
| 自动更新 agent | **保留，追 npm latest** | 不要钉死 @version 装包 |
| Hermes/Cursor 一键下载安装 | **还没有** | 只是列表里多几个；你没用到就不影响 |
| binary 缓存、会话 meta 花活 | **还没有** | 不影响当前 4 个主力 agent 聊天 |

### 体积预算（exe 尽可能小）

| 阶段 | 大约 | 说明 |
|------|------|------|
| cargo release（LTO+strip+opt-z，**无 ring**） | **~7 MB** | `ureq` + Windows `native-tls`/`schannel`，不要 rustls/ring |
| **+ UPX `--best --lzma`** | **~1.2–1.3 MB** | 发货口径；参考 `D:\垃圾\Marionette-upx-ringless.exe` |
| 前端 dist | ~0.6 MB | 嵌在壳内 |

| 纪律 | |
|------|--|
| **禁止重新引入 ring / rustls** | 会涨包且 UPX 后仍难回到 1.2MB |
| **禁止** tokio / zip / reqwest / 下载缓存 | 对齐 harness 时只加表驱动逻辑 |
| 发货脚本 | `build-portable.bat`：有 UPX 则压，无则提示 |
| agent | PATH + npm 自动更新，**不**打进 exe |

---

## 0. 一句话

向 Codeg **对齐协议与 agent 启动能力**，**不抄 UI / 交互壳**；对齐过程中 **最高优先级保护**：

> **同一个对话窗口（session）可反复切换 agent，并共享项目级 Skill 与 MCP。**

评论批注、图片编辑等 compose 侧能力默认冻结，不随对齐改动。

---

## 1. 目标与非目标

### 1.1 要对齐（In scope）

| 支柱 | 含义 |
|------|------|
| **1. ACP 完整性** | 客户端方法、能力广告、回包形状、挂起请求生命周期与 Codeg 已验证的 wire 一致（按 agent 裁剪） |
| **2. Agent harness 完整性** | 内置 agent 清单与 Codeg 同级（Claude / Codex / OpenCode / Grok / Cline / Pi / Kimi / …）；能装、能起、该开的能力真能用 |

包含但不限于：

- `elicitation/create`（Codex Plan / request_user_input 等）
- Grok `_x.ai/ask_user_question`、`_x.ai/exit_plan_mode` 及正确 `outcome` 回包
- `terminal/*`、`fs/*`、`session/request_permission`
- 按 agent 的 `clientCapabilities`（如仅 Codex 广告 `elicitation.form`）
- 工具名 / plan 工具 **语义归一化**（纯数据，无 UI）
- 断开 / 超时 / stop：`keep_planning` / cancel / decline 策略
- Agent registry：分发（npx / binary / uvx）、preflight、env/MCP 策略表

### 1.2 明确不对齐（Out of scope）

| 不抄 | 原因 |
|------|------|
| Codeg 消息气泡、ai-elements、虚拟列表、主题 | 产品壳是 Marionette 自己的 |
| Plan / Ask / 审批卡的 **视觉与布局** | 只对齐 **事件契约与回包语义**；现有 Card 可继续用 |
| codeg-mcp 委托 / 多 agent 伴生平台 | 除非另开题；与我们的 project lend 模型不同 |
| remote/web transport、宠物、office 等 | 无关 |
| 整仓换成 sacp 异步架构 | 非必须；现有同步 host 上扩展即可 |

### 1.3 「卡交互语义」指什么（非 UI）

- 事件名与字段（如 `question/prompt`、`plan/approval`、`permission/prompt`）
- 用户动作 → Rust 侧 JSON-RPC 回包形状
- 超时 / stop / 进程死亡时的默认 outcome  
→ **协议状态机**，不是换 React 组件。

---

## 2. 产品硬约束（不可破坏）

实施任何 PR 前对照本节。违反即设计错误。

### 2.1 P0 — 同窗切换 agent + 共享 Skill / MCP

这是 Marionette 与「单 agent 客户端」的根本差别，**高于**「多抄几个 agent」。

```text
同一 dialog（sessionId 不变）
  ├─ Clean transcript 共用（切 agent 只 seal 回复 + 换进程，不清历史）
  ├─ 项目级 inventory + 用户勾选（哪些 MCP / Skill 开启）
  ├─ 新 agent session/new 时 mcp_payload_for_agent(...) 再注入
  └─ Skills 可走 project_context_prompt 进入上下文
```

| 规则 | 说明 |
|------|------|
| **session 身份稳定** | 切换只改 `session.agentId` + 停旧 ACP 进程 + 再 warm；**不新建 session 顶替对话** |
| **历史共用** | `handleAgentChange`：seal 未完成 Reply、flush transcript、保留 live/Clean 事件 |
| **MCP/Skill 项目级** | `context_inventory` 扫描 + selection；**不是** Codeg 式每连接只塞自家 MCP |
| **按 agent 再 lend** | 每次 `start` / `session/new` 按 **当前 agent** 能力与 policy 注入；换 agent = 换进程 + 同一勾选池再投递 |
| **policy 例外不否决模型** | 如 OpenClaw 禁止非空 `mcpServers`：该 agent 不注入，**不删除**项目共享菜单 |

实现锚点（现状，对齐时必须保留钩子）：

- 前端：`App.tsx` → `handleAgentChange`
- 后端：`context_inventory::mcp_payload_for_agent`、`skills_prompt_for_agent`
- 启动：`acp.rs` `session/new` 的 `mcpServers` + agent 特判（Grok trust、OpenCode permission env 等）

**Codeg 对齐只增强「换上来的 agent 协议能不能干活」；不得改成「一 agent 一孤岛、丢掉项目 lend」。**

### 2.2 P0 — 现有 Marionette 特判与模块

用 **registry + policy 钩子包住**，禁止整文件替换时抹掉：

| 模块 / 行为 | 角色 |
|-------------|------|
| `context_inventory` | 共享 MCP/Skill、workspace 外路径、Grok folder trust |
| `handoff` | 切 agent 时备忘（不自动发送） |
| 委派子会话 | `parent_session_id` / origin / `create_child_session` |
| Grok | trust、billing `_x.ai/billing`、ask / exit_plan |
| OpenCode | `opencode_permission_env`、目录放宽 |
| Session prefs | model / mode / effort / always-approve 落盘 |
| Transcript | `write_transcript` / `load_transcript` 兼容旧 JSONL |
| Todos / git / app_update | 与 agent 协议正交，默认不碰 |

### 2.3 P1 — 默认冻结的前端能力（对齐不碰契约）

| 能力 | 路径 | 为何安全 |
|------|------|----------|
| **评论批注 (quote pin)** | `quoteComment` → `formatPinsForSend` → 纯文本进 prompt | 协议只见文本 |
| **图片编辑 (ImageAnnotator)** | 本地改图 → `imagePaths` → `send_acp_prompt` | 协议只见图片 ContentBlock |
| Clean View 卡片折叠 / 大纲 | `SessionView`、`acpTranscript` | 消费事件，不定义协议 |
| Force web search 包装 | `forceWebSearch` | 改字符串 |
| Composer 壳、侧栏、主题 | 各组件 | 产品壳 |

对齐时：**只增不改名** 现有 `acp-event` kind 与 `respond_*` / `send_acp_prompt` 契约；批注与图片 **零需求改动**。

### 2.4 事件与 API 兼容

- 新增 kind 可以（如 elicitation 映射到既有 Ask/Permission 或新 kind）
- **禁止** 静默改掉：`permission/prompt`、`question/prompt`、`plan/approval`、`session/update` 已有字段含义
- 前端 `respondAcpPermission` / `respondAcpQuestion` / `respondAcpPlanApproval` 保持可用；新路径可并行

---

## 3. 现状摘要（对齐前）

### 3.1 已有 agent（`models::AgentConfig::defaults`）

- OpenCode · Codex · Claude Code · Grok Build  

### 3.2 Codeg 内置 12（`acp/registry.rs`）

Claude Code · Codex · Gemini · OpenClaw · OpenCode · Cline · Hermes · CodeBuddy · Kimi Code · Pi · Grok · Cursor  
（+ 用户 custom registry）

### 3.3 ACP 客户端方法

| 方法 | Marionette 源码 | 备注 |
|------|-----------------|------|
| `fs/*` | 有 | |
| `terminal/*` | 有 | 须确认运行二进制已更新 |
| `session/request_permission` | 有 | |
| `_x.ai/ask_user_question` | 有 | Grok；`outcome` 回包 |
| `_x.ai/exit_plan_mode` | 有 | Grok Plan 退出；须真机验收 |
| `elicitation/create` | **无** | Codex 关键空洞 |
| 按 agent 的 capabilities 裁剪 | **弱** | 全员同一 initialize |

### 3.4 已知根因（历史）

Grok Plan 批准失败 *client disconnected*：未实现 `_x.ai/exit_plan_mode`，agent 收到 -32601。  
源码已按 Codeg wire 补 handler；**以重载后真机验收为准**。

---

## 4. 目标架构（逻辑）

在现有 Tauri 同步 ACP host 上演进，不强制迁移 Codeg 全量 async 栈：

```text
src-tauri/src/
  acp/                    # 可从单文件 acp.rs 逐步拆出
    client_methods.rs     # permission / ask / exit_plan / elicitation / fs / terminal
    capabilities.rs       # 按 agent 生成 clientCapabilities
    question.rs           # ask + elicitation classify + response builders
    plan_approval.rs      # exit_plan 纯逻辑
    terminal_runtime.rs   # 已有
    fs_runtime.rs
  agents/                 # harness
    registry.rs           # 内置 meta + custom 钩子
    launch.rs             # PATH / npx / uvx / binary
    policy.rs             # env、MCP 是否转发、与 context_inventory 的交界
    preflight.rs
    install.rs            # 可分期

context_inventory.rs      # 保持：项目级共享 Skill/MCP（P0）
handoff.rs                # 保持
```

前端：

```text
src/lib/toolCallNormalize.ts   # 纯函数：别名 / kind（从 Codeg 表移植语义）
# 现有 AskQuestionCard / PermissionDialog / PlanApprovalCard 只接事件，不换皮
```

原则：

1. **Wire / 策略 / 别名表** 对齐 Codeg 已验证行为  
2. **Harness** 对齐「有哪些 agent、怎么起、能力开关」  
3. **UI** 只保证事件可驱动现有组件  

---

## 5. 分阶段实施

### Phase A — ACP 协议闭环（先稳现有 4 agent）

1. **Elicitation（Codex）**  
   - 迁 Codeg `question::classify_elicitation` 与 response builders  
   - 处理 `elicitation/create`  
   - **仅 Codex** 广告 `elicitation.form`（避免 Claude 双通道）  
   - 前端复用 Ask / Permission 契约  

2. **Grok 扩展验收**  
   - exit_plan / ask / terminal 真机 + unit test  
   - 断开 / 超时策略与 Codeg 一致  

3. **clientCapabilities 表驱动**  

4. **工具 / plan 语义归一化（TS）**  
   - 不绑定 UI 组件  

**验收**：Grok Plan 可批准；Codex Plan/提问能回；四 agent 终端不 -32601；**切 agent + MCP 勾选仍生效**。

### Phase B — Agent registry + 扩军

1. `AcpAgentMeta` / Npx·Binary·Uvx 模型（版本策略建议先钉 Codeg 当前 pin）  
2. B1：PATH/npx 能起即收录；B2：binary/uvx 缓存与安装  
3. **policy 表** 挂接现有 lend（见下表）  
4. 分批加 agent：  
   - B1：Cline、Gemini、Kimi、CodeBuddy  
   - B2：Pi、Hermes  
   - B3：OpenClaw、Cursor  
   - B4：Custom registry（可选）  

**每 agent 验收清单**：install/探测 · session/new · 流式 update · permission · terminal · fs · Ask/Plan（若有）· stop 清 pending · **MCP：该开开、该关关且项目勾选仍生效**。

### Phase C — 会话语义补强（仍非 UI）

- Codex goal/compaction/retry 等 meta → 现有 event 类型映射  
- Claude `subagent-transcript` meta  
- 不默认做：委托 MCP 平台、fork/resume 大改、steering  

---

## 6. 按 agent 的 harness 要点（与共享上下文的交界）

| Agent | 协议/启动要点 | 与共享 MCP/Skill |
|-------|----------------|------------------|
| Grok | trust、exit_plan、ask、billing | lend + folder trust |
| Codex | elicitation.form、依赖 `codex` CLI | lend；注意 MCP 过滤 env |
| Claude | subagent-transcript；依赖 `claude` | lend |
| OpenCode | permission env、额外目录 | lend（已有） |
| OpenClaw | **禁止** 非空 mcpServers | 共享菜单可显示，**本 agent 不注入** |
| Pi | preflight `pi`；wire 上不转发 MCP | 同左 |
| Hermes | uv/python 钉版本 | lend |
| Cursor | 整树 binary；env 凭据 | lend |
| Cline / Gemini / Kimi / CodeBuddy | 标准 npx ACP | lend |

---

## 7. 从 Codeg 取什么、不取什么

**当规格读、抽逻辑：**

- `acp/plan_approval.rs` — exit_plan wire  
- `acp/question.rs` — ask + elicitation  
- `acp/connection.rs` — `build_client_capabilities`、handlers  
- `acp/registry.rs`、`binary_cache.rs` — agent 清单与分发  
- `lib/tool-call-normalization.ts`、`lib/plan-parse.ts` — 语义表  

**不搬：** `components/message/*`、ai-elements、Next 壳、codeg-mcp 全量委托架构。

---

## 8. 实施纪律

1. **先保护 P0，再扩协议，再扩 agent 数量。**  
2. 改 `acp` 启动 / `session/new` / MCP 注入时：手工或脚本验证  
   - 同 session 从 A 切到 B  
   - 历史仍在  
   - 侧栏 MCP 勾选在 B 上仍按 policy 注入（或合法跳过并有日志）  
3. 前端产品路径（SessionView / Composer 批注图片）**默认只读**。  
4. 声称完成必须有：编译证据 + 至少一条真机路径（Grok Plan 或 Codex elicitation）。  
5. 版本：建议 **先钉 Codeg 同 pin**，再谈浮动 latest。

---

## 9. 决策记录（已拍板）

| 项 | 决定 |
|----|------|
| UI / 交互 | **不抄** Codeg |
| 协议扩展（elicitation、exit_plan、ask、归一化、断开语义） | **要做** |
| 产品主轴 | **同窗切 agent + 项目级共享 Skill/MCP** |
| 批注 / 图片 | **对齐不碰** |
| 特判模块 | **registry 包住，禁止抹掉** |
| codeg-mcp 委托平台 | **默认不做** |

待实施时再确认（可改）：

- 版本钉死 vs latest  
- binary_cache 是否 Phase B1 就上  
- MVP 是否一次上满 12 agent 或按 B1–B3 批次  

---

## 10. 验收总表（对齐「完成」的定义）

- [~] Grok：Plan 审批卡已出现且可操作（批准/改/弃）；完整 ask / terminal / 断连策略仍待勾  
- [x] Codex：`elicitation/create` 源码已接（approval→权限卡，questions→Ask 卡）；真机待验  
- [x] clientCapabilities 按 agent（Codex form / Claude subagent-transcript）  
- [x] 工具名归一化（`toolCallNormalize.ts`）  
- [x] Agent 列表扩展（Cline/Gemini/Kimi/CodeBuddy/Pi/Hermes/OpenClaw/Cursor）+ MCP wire policy  
- [ ] 现有 4 agent：通用 ACP 不因缺方法挂死（真机）  
- [ ] **同 session 切换 agent ≥2 次，历史保留，MCP/Skill 勾选行为正确**  
- [ ] 批注发送、图片附件发送回归通过  
- [ ] 新增 agent（按批次）满足第 5 节清单  
- [x] 无 Codeg UI 依赖引入  

### 10.1 本轮落地文件（2026-07-30）

| 文件 | 作用 |
|------|------|
| `src-tauri/src/elicitation.rs` | form elicitation 分类与回包 |
| `src-tauri/src/agent_registry.rs` | capabilities / MCP wire policy |
| `src-tauri/src/acp.rs` | elicitation handler、按 agent initialize、MCP skip |
| `src-tauri/src/models.rs` | 扩展内置 agent |
| `src/lib/toolCallNormalize.ts` | 工具名归一化 |
| `src/lib/acpTranscript.ts` | 接入归一化 |

---

## 11. 文档关系

| 文档 | 关系 |
|------|------|
| `docs/05-next-roadmap.md` | 总路线；本文为 Codeg/ACP 专篇 |
| `docs/06-context-bridge-and-subagents.md` | Skill/MCP/子会话调查；**共享 lend 与本文 P0 一致，实施以本文约束为准** |
| `docs/superpowers/specs/*` | 历史功能规格；冲突时以 **本文硬约束 + 现行代码** 为准 |

---

*本文是实施前的对齐规格。开干时按 Phase A → B → C 拆 PR，每 PR 对照第 2 节硬约束自检。*
