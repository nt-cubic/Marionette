# Marionette 推进路线（2026-07-24 续）

> 状态：进行中  
> 依据：`docs/01–04`、`docs/superpowers/specs/*`、以及近期实现与讨论  
> 目标：在 **不扩大成 Agent 平台 / 不重写全 Rust** 的前提下，把本地多 Agent 壳做到好用、像 Zed 一样克制。

---

## 0. 产品底线（再确认）

1. **Clean View 是产品主界面**；Raw Terminal 是权威旁路，随时可切，不重启进程。  
2. **每个对话框绑定一个 Agent**（`session.agentId` 落盘）；换 Agent = 换绑定 + 停旧进程，不是热改进程身份。  
3. **ACP 优先**（OpenCode / Codex / Claude / Grok 等）；PTY 仅作 fallback。  
4. **懒启动 ACP**：不在点开会话时卡 UI；focus/send 时后台 warm；**不打断输入法**。  
5. **不做** API key 云平台、MCP marketplace、自研 Agent runtime。

---

## 1. 当前已完成（相对文档的“现在”）

### 1.1 核心壳

| 项 | 状态 |
|----|------|
| Tauri + React 主布局 | ✅ |
| 项目 CRUD / session 列表 | ✅ |
| 多 Tab、Clean ↔ Raw | ✅（**默认 Clean**） |
| Raw PTY + xterm | ✅ |
| ACP JSON-RPC（initialize / session/new / prompt / cancel / set_config） | ✅ |
| Composer：model / mode / effort（按能力协商） | ✅ |
| session ↔ agent 强制绑定 + 落盘 | ✅ |
| ACP 懒启动 + async handshake（减轻卡顿） | ✅ |
| IME 保护（composition 期间不 warm） | ✅ |

### 1.2 Clean / 体验

| 项 | 状态 |
|----|------|
| You / Thinking / Tool / Reply 卡片 | ✅ |
| 空态（非 “empty” 竖排 bug） | ✅ |
| 细滚动条、尽量无横向滚动 | ✅ |
| 对话 transcript JSONL 读写 | ✅ |
| session 标题自动命名（首条消息 / session_info_update） | ✅ |
| 左侧 Search threads（标题 + transcript） | ✅ |
| Claude 未登录横幅 + **一键 Sign in**（`claude auth login`） | ✅ |
| Usage 面板（ACP usage + 部分 provider 余额） | ⚠️ 半成品 |
| OpenCode 按当前模型查余额 | ⚠️ 部分 provider；Go 无公开 remaining API |

### 1.3 明确未做（文档 ❌ / 讨论中的下一步）

| 项 | 状态 |
|----|------|
| Handoff（`.marionette/handoff.md` + 换 Agent prefill） | ✅ P1 |
| Git / Changed Files / Diff View | ✅ 列表 + 只读 diff |
| 权限请求 UI（ACP 现自动 allow） | ✅ 用户确认 + 120s timeout |
| 完整错误分类体系 | ✅ auth/command/timeout/model/network |
| Open in editor / 文件树 | ❌ |
| 无边框窗口 + 左上搜索拖窗（Zed 风 chrome） | ✅ P1 |
| Tab 切换 mode（OpenCode 习惯） | ✅ P2-UX |
| Clean 用户消息目录条（outline） | ✅ 仅 You |
| 双击 Esc 中断 agent | ✅ ACP cancel / PTY Ctrl+C |
| 编辑已发消息（截断后续 + 重发） | ✅ 壳层 edit&resend |
| ASP 产品层统一（文档有，代码仍 ad-hoc） | ❌ |
| 全 Rust UI 重写 | ❌ **不建议当前做** |

---

## 2. 推荐推进顺序（从“马上能感知”到“文档 MVP 欠账”）

### P0 — 已完成（本轮）

- [x] 对话保存 / 恢复（transcript JSONL）  
- [x] session 标题自动更新  
- [x] 左侧 Search threads 可用  
- [x] Claude 登录提示 + 一键登录  
- [x] Clean 默认视图  
- [x] 空态 / 滚动条 / 输入法卡顿 等体验修补  

### P1 — 下一阶段（建议按序）

#### Step A：Zed 风窗口 chrome（1–2 天） — ✅

1. [x] Windows：`decorations: false`  
2. [x] 左侧顶栏 = **真搜索** + 拖拽区（`data-tauri-drag-region`）  
3. [x] 右上自绘 min / max / close  
4. [x] **不再**保留一条“Marionette”固定大标题栏  

验收：全屏后像编辑器壳，左上可搜可拖，无多余顶栏。

#### Step B：Handoff（文档 MVP 硬要求） — ✅

1. [x] 从当前 Clean transcript 生成 `.marionette/handoff.md`  
2. [x] 切换 Agent 时 **prefill Composer**，**不自动发送**  
3. [x] 右侧 Information 露出 Handoff 摘要入口  

验收：Claude → OpenCode（或反向）能带着上下文继续，用户点一次发送。

#### Step C：Changed Files / 轻量 Git — ✅

1. [x] 对当前 project `git status --porcelain`  
2. [x] 右侧列表：路径 + 状态  
3. [x] 可选：点开看 diff（只读）  

验收：有改动的仓库右侧不为空。

#### Step D：权限与错误 — ✅

1. [x] ACP `session/request_permission` → 用户确认对话框（不再全 auto-allow）  
2. [x] 错误分类：auth / 缺命令 / timeout / model 不支持 / 网络  
3. [x] Clean 与 Composer 统一展示  

验收：危险工具会弹窗；未登录只出现可操作的登录态，不“静默无回复”。

### P2 — 体验增强（已拍板语义，待实现）

> 讨论结论（2026-07-24）：优先键盘与长会话导航；**不承诺改 agent 内部记忆**，壳层诚实降级。

#### P2-UX-1：Tab 切换 mode（学 OpenCode） — ✅

| 项 | 约定 |
|----|------|
| 行为 | Composer 内 **Tab / Shift+Tab** 循环 mode |
| 底层 | 复用 `session/set_config_option` |
| 无 mode | 不拦截 Tab（允许焦点移动） |
| 反馈 | 轻 toast `Mode · …` |

#### P2-UX-2：用户消息目录条（conversation outline） — ✅

| 项 | 约定 |
|----|------|
| 索引 | **仅 You**（≥2 条时显示） |
| UI | 右侧细点轨；hover 摘要；点击滚动 + 高亮 |

#### P2-UX-3：双击 Esc 中断任务 — ✅

| 项 | 约定 |
|----|------|
| 手势 | ≤400ms 连按两次 Esc |
| 底层 | ACP `cancel`；PTY `\x03` |
| 优先级 | 权限弹窗 / diff / 项目对话框 / Composer 菜单 先关 |

#### P2-UX-4：编辑已发消息 = 截断后续 + 重发 — ✅

| 项 | 约定 |
|----|------|
| 语义 | You 卡 **Edit** → 截断后续 → `session/prompt` / PTY 重发 |
| 本地 | `liveEvents` + transcript JSONL 同步截断 |
| 诚实 | 文案「Resend from here / truncates…」；不宣称改 agent 记忆 |

---

### P2 — 其它打磨（原清单）

1. Usage：Go 无 API 时诚实 Unavailable；会话内 context 更稳  
2. Diff / Logs 真面板  
3. Open in Zed / VS Code  
4. 虚拟列表（超长会话；与 outline 锚点兼容）  
5. ASP 事件命名收敛（内部文档对齐）  
6. [x] Composer **拉高**（Expand 切换；Esc 可收回）  
7. Composer MD 预览输入 — **明确不做**  
8. Raw Terminal 主路径 — **倾向弱化**（用户为 Clean 而来；真 TUI 用系统终端即可）。Open in CLI 若做，另立条目，不与 Clean 双主路径  
9. [x] **活动 / loading 指示**（L1–L3）：Tab 脉冲、顶栏 Working + last update、You 后幽灵 Waiting、20s 无事件 stale 文案、Composer busy 条  

### 产品备忘：Clean vs 终端（2026-07-24）

- 下载 Marionette 的核心动机是 **Clean 多 Agent 壳**，不是再包一层 PowerShell。  
- 内嵌「ACP wire dump」式 Raw 价值低；**双入口（本 session 锁终端 / 锁 Clean）** 与产品目标矛盾。  
- 若未来做终端：优先 **Open in 原厂 CLI + handoff 分叉**，不与 ACP Clean 假装同一会话。  
- 当前：可保留弱 Raw 作排障，但不作为主卖点。  

### 产品备忘：Changed Files vs Handoff 粒度（2026-07-24 拍板）

| 能力 | 粒度 | 理由 |
|------|------|------|
| **Changed Files（git）** | **项目** | 真相是共享工作区 `git status`；多对话框改同一棵树，应看见同一份列表 |
| **Handoff** | **对话框（session）** | 交接的是该对话的 transcript；项目级单文件会多 Tab 互相覆盖 |

**现状与后续：**

- Changed Files：保持 `get_changed_files(projectId)` + 项目 cwd。文案可强调 *Project · git*。  
- Handoff：**已 per-session** — `.marionette/handoff/{sessionId}.md`，并同步一份最新快捷副本 `.marionette/handoff.md`。

- **不做**：把 git Changed Files 拆成「每对话框一份」（假 git）。  
- **可选另做**：「This session」改动（从 tool_call / 文件事件推），与 git 列表分开展示、不同名。  

### 明确不做（当前）

1. 整应用移植纯 Rust UI（egui/GPUI）— 成本过高，xterm/MD 要重造  
2. 云同步 / 账号体系 / MCP 商店  
3. 伪造 Claude 历史版本号列表（只信 ACP `configOptions`）  
4. **协议级「改 agent 内部历史」** — ACP 无统一 API；只做壳层截断 + 重发  
5. **目录条索引 Thinking/Tool** — 噪音大；MVP 仅 You  

---

## 3. 技术约定（推进时遵守）

### 3.1 视图

- 新建 session：`viewMode = "clean"`  
- Raw 仅用户主动切换；切换不杀 ACP/PTY  

### 3.2 启动与输入

- 禁止在「打开 session」时同步阻塞握手  
- `start_acp_session` 必须 async + blocking pool  
- Composer：`compositionstart/end` 期间不 warm、不抢 focus  
- warm：focus 一次即可，不要每个 keypress  

### 3.3 状态真相

| 真相 | 字段 / 存储 |
|------|-------------|
| 这个对话框是谁 | `session.agentId`（磁盘 + UI） |
| Clean 历史 | `.marionette/transcripts/{id}.jsonl` |
| 标题 | `session.label` |
| 模型列表 | **仅**当前 session 的 ACP caps，禁止跨 session 泄漏 |
| Git 变更 | **项目**工作区（非 session） |
| Handoff（目标） | `.marionette/handoff/{sessionId}.md`（当前仍为项目级单文件，待迁） |
| Model / Mode / Effort | session 清单 `preferredModel/Mode/Effort/EffortId`；caps 仍有则恢复 `set_config`；换 Agent 清空 |
| Agent 对话记忆 | UI=transcript JSONL；ACP 每次 `session/new` 无记忆 → **冷启动首次 prompt 注入本地历史**（非真正 session/load） |

### 3.5 交互约定（P2-UX）

| 交互 | 约定 |
|------|------|
| Tab（Composer） | 循环 mode；无 mode 则忽略 |
| Esc×2 | 中断 turn；Esc×1 优先关浮层 |
| Outline | 仅 `user_message`；hover 摘要 + click 滚动 |
| 编辑 You | 截断该条之后 + 重发；工具副作用不自动回滚 |

### 3.4 布局方向（Zed）

```text
┌ search / drag ──────────────────────────────── controls ┐
│ projects & sessions │ Clean (primary) │ Information    │
│                     │ Raw (toggle)    │ Usage / later  │
├─────────────────────┴─────────────────┴────────────────┤
│ Composer: bound agent · model/mode/effort · send         │
└──────────────────────────────────────────────────────────┘
```

---

## 4. 建议的下一刀（执行清单）

**P1 + P2-UX 已完成。** 当你说「继续」时，可按需插队：

1. **Handoff per-session**（`.marionette/handoff/{sessionId}.md`，避免多 Tab 覆盖）  
2. Usage 诚实态 / Diff 面板 / Open in editor / 虚拟列表…  
3. （可选）This session 改动列表 — **不要**拆 git 为 per-dialog  

每完成一步：

每完成一步：

- 可手动验收  
- 本文件对应 checkbox / 状态勾掉  
- 不扩 scope（尤其 Edit 不做文件自动 revert）  

---

## 5. 已知坑（避免再踩）

1. Claude：`effort` 仅部分模型有；不要 invent config option。  
2. Claude 模型列表是官方 **别名**（opus/sonnet/haiku…），版本在 description。  
3. Clean 空态不要用 `event-card` 双列网格（会竖排挤字）。  
4. 同步 Tauri command 做 ACP 握手 → WebView/IME 卡死。  
5. Composer `key={sessionId:agentId}` 防止 caps 串话。  
6. 无边框窗：`data-tauri-drag-region` **不可包住** min/max/close 按钮（会吞点击）。  
7. 编辑重发 ≠ 撤销磁盘副作用；需要时靠 git / Changed Files 人工处理。  

---

## 6. 相关文档

| 文档 | 用途 |
|------|------|
| `docs/01-product-plan.md` | 产品范围与底线 |
| `docs/02-program-design.md` | 架构 |
| `docs/03-implementation-guide.md` | 里程碑施工 |
| `docs/superpowers/specs/2026-07-24-agentshell-direction-design.md` | ACP/PTY/ASP 方向 |
| `docs/superpowers/specs/2026-07-24-agentshell-asp-protocol.md` | ASP 草案 |
| `docs/research/zed-lightweight-clone.md` | Zed 交互标尺 |
| **`docs/05-next-roadmap.md`（本文）** | **当前推进清单** |

---

*最后更新：2026-07-24 — P0/P1/P2-UX 完成；拍板：git Changed Files=项目级，Handoff=应对话框（待迁路径）。*
