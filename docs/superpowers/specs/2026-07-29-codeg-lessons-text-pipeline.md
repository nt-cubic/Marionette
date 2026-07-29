# 从 Codeg 学什么：文本 / Thinking 管线（克制版）

> 日期：2026-07-29  
> 状态：**P0 代码已合入，待你本地验收**  
> 对照：`D:\Myself\codeg-main`（Codeg）  
> 底线：Marionette 是**轻量本地多 Agent 壳**，不是 Codeg 级工作台。

---

## 当前进度（看这节就行）

### 已完成（代码里）

| # | 步骤 | 落点 | 状态 |
|---|------|------|------|
| 1 | 掏空 `collapseIntermediateAssistantAsThought`（不再把 Reply 改写成 Thought） | `acpTranscript.ts` | ✅ |
| 2 | 去掉 tool 前「上一句 Reply → Thought」 | `acpTranscript.ts` | ✅ |
| 3 | 关掉 `§` 软判定开 Thought 卡（只信协议 thought 流） | `acpTranscript.ts` | ✅ |
| 4 | Thought UI：流式「Thinking…」展开；结束「Thought · Ns」默认折叠 | `SessionView.tsx` + css | ✅ |
| 5 | 规范文档（立场 / 不抄清单 / 验收场景） | 本文 | ✅ |

### 你现在要做（验收，不写代码）

| # | 动作 | 怎样算过 |
|---|------|----------|
| A | **重启**桌面 Marionette（加载新前端） | — |
| B | 用 **会 thinking** 的 agent（Claude / OpenCode 等）问一个会先想再答的问题 | 先出现 Thinking（可展开），再出现 **Reply**；两者分开，Reply 不进 Thinking |
| C | 多 tool 回合：让 agent 改几个文件 | tool 之间若有短说明，结束后仍是 **Reply**，不是 Thought |
| D | 有图的一轮：拖图标注再发 | You 卡图+叠加仍正常；不回归闪退（回前台清通知） |
| E | 若仍「吞内容」 | 记：哪个 agent、是否 thinking 模式、大致现象 → 再开修 |

**过了 A–D → 本规范 P0 结案。**  
不过 → 带着现象回来，再针对性补。

### 验收通过后，才考虑的下一步（按需，不是默认要做）

| 序 | 项 | 何时做 | 状态 |
|----|-----|--------|------|
| 1 | 流式 Markdown 加固（半截 fence / 列表抖） | Reply **渲染**仍丑/抖时 | ⬜ 可选 |
| 2 | You 卡路径/引用 segment 渲染 | 想和药丸心智完全统一时 | ⬜ 可选 |
| 3 | 其它体验债 / bug | 真机用出来再排 | 持续 |
| — | 「联网」强制检索按钮（提示词前缀） | 用户点名 | ✅ 已做（`forceWebSearch.ts` + Composer） |

**默认不排：** Codeg 全量功能、Streamdown 全家桶、全文文件批注 IDE。

---

## 0. 产品立场（先钉死）

Codeg 功能更全（会话导入、多端、Server/Docker、DB、委派编排、Office/Science 捆绑、虚拟列表……）。  
**我们不跟全量，也不跟它的重量。**

| 我们要 | 我们不要 |
|--------|----------|
| 思考 / 正文 / 工具 **分轨清晰** | 把中间 Reply 事后改写成 Thought |
| 流式文本 **稳、不抖、不吞** | Streamdown + shiki + mermaid + math 全家桶首屏 |
| 可折叠 Reasoning 的**语义** | AI SDK Elements 整包搬迁 |
| 继续克制壳 | 会话聚合市场、远程协作、重工作台 |

**一句话：** 抄「消息类型与呈现纪律」，不抄「产品广度」。

---

## 1. 对照结论

### 1.1 Codeg 做对了什么

1. **协议类型优先**  
   `agent_thought_chunk` → `ContentBlock::Thinking`；正文 → `Text`。  
   适配层再变成 `reasoning` / `text` part。**几乎不靠内容启发式猜「这是不是思考」。**

2. **渲染按 part 分发**  
   `reasoning` → 专用 Reasoning（流式展开、结束后收起、时长文案）；  
   `text` → 流式 Markdown；  
   tool → 分型卡片。

3. **Markdown 为流式服务**  
   用 Streamdown 一类为增量渲染设计的管线；重插件懒加载。

4. **用户消息 segment**  
   文本与 `file:` / 引用 badge 拆开渲染（和「路径药丸」同方向）。

### 1.2 Marionette 当前痛点（与「吞 think」直接相关）

| 机制 | 位置 | 风险 |
|------|------|------|
| `§` 等 **软判定** thought | `acpTranscript.ts` | 正文误入 Thinking |
| **`collapseIntermediateAssistantAsThought`** | 回合结束改写 | 把本回合中间的 `assistant_message` **降级成 thought** → 用户以为「吞了」 |
| Thought UI 弱 | SessionView | 无「思考中 / 用了多久」语义 |
| `react-markdown` 全量挂载 | MarkdownBody | 流式半截 fence / 列表易抖（次要） |

**根因判断：** 结构层先「猜」再「改写」> 呈现层不够漂亮。

---

## 2. 明确不抄（防膨胀）

下列 Codeg 能力 **本规范范围外**，提出时可直接指这里：

1. 全量会话导入 / 跨 agent 历史索引  
2. Server / Docker / 移动端 / 远程连接  
3. SeaORM 级本地库与复杂 migration  
4. Office / Science skills 捆绑  
5. 首屏加载 shiki + katex + mermaid  
6. 虚拟化超长线程（单会话真到上千条再单独立项）  
7. 完整委派编排引擎 / 协作工作台  
8. 文本·代码·PDF 全文批注 IDE（图片批注已够；L1 选区另议）

---

## 3. 要抄的目标态（最小）

### 3.1 事件语义（硬规则）

```
sessionUpdate 含 thought / agent_thought_chunk / thinking 块
  → 永远是 thought 卡（可合并相邻 thought）

sessionUpdate 是 agent_message / 明确 assistant 流
  → 永远是 assistant_message（Reply）
  → 禁止在回合结束把它改写成 thought

§ / 标点启发式
  → 默认关闭；若保留，仅允许「接到已有 thought 卡末尾的噪声 token」
  → 禁止仅凭 § 新开 thought 或把整段 Reply 判成 thought
```

### 3.2 回合结束

| 旧 | 新 |
|----|-----|
| `collapseIntermediateAssistantAsThought` 把中间 Reply 降级 | **删除或改为 no-op**（保留函数名可标 `@deprecated` 空实现，避免全仓改调用） |
| 多个 thought 卡碎 | 可选：`coalesceAdjacentThoughts` **仅合并相邻 type=thought** |
| 未封口的 Reply | `sealOpenAssistantReplies` **只盖 durationMs**，不改 type |

### 3.3 Reasoning UI（轻量自研，不引 Elements）

Thought 卡：

| 状态 | 展示 |
|------|------|
| 流式中 | 默认展开；标题「Thinking…」（可 shimmer 或脉冲点） |
| 已结束 | 默认折叠；标题「Thought · Ns」或「Thought」 |
| 空内容 | 不渲染或极短占位，不占一坨空卡 |

**不做：** 独立脑图、多分支 reasoning、与 tool 交织的复杂时间轴（Codeg 级）。

### 3.4 Markdown（第二阶段）

| 阶段 | 动作 |
|------|------|
| A（本规范必做） | 先修类型管线；Markdown 仍用现有 `MarkdownBody` |
| B（可选 follow-up） | 仅 assistant 流评估 Streamdown **或** 加固半截 fence；**禁止**首屏拉 mermaid/math |

### 3.5 用户消息 segment（可选 follow-up）

与图片药丸一致的心智：发送后的 You 卡可把路径/图附件当 chip 渲染。  
**不在本规范 P0 阻塞项。**

---

## 4. 落点（施工地图）

| 文件 | 动作 |
|------|------|
| `src/lib/acpTranscript.ts` | 收紧 `extractAcpUpdateText` thought 判定；削弱/删除 collapse；保留 coalesce 仅 thought |
| `src/app/App.tsx` | 回合结束仍调用 seal；collapse 改为无害或移除 |
| `src/components/SessionView.tsx` | Thought 卡 UI：流式 / 结束 / 时长 |
| `src/styles/app.css` | Reasoning 折叠样式（克制） |
| `docs/05-next-roadmap.md` 或本文件状态 | 完成后勾选 |

**零 Rust 改动（P0）。** 协议侧已有 thought chunk；问题在前端归类与改写。

---

## 5. 施工步骤总表（含状态）

| 步 | 内容 | 验收 | 状态 |
|----|------|------|------|
| **1** | 掏空 `collapseIntermediateAssistantAsThought` 降级 | tool 间 assistant 短句回合后仍是 Reply | ✅ 已做 |
| **2** | tool 前不再 demote Reply→Thought | 开 tool 不吞上一句正文 | ✅ 已做 |
| **3** | `§` 不再单独开 Thought；噪声仅可并入**已有** thought | 正文标点/`§` 不误开 Thinking | ✅ 已做 |
| **4** | Thought UI：流式展开 + 结束折叠 + 时长 | 标题 Thinking… / Thought · Ns | ✅ 已做 |
| **5** | **本地验收**（见文首「你现在要做」） | A–D 通过 | ⬜ **当前步** |
| **6** | （可选）流式 Markdown 加固 | 半截 fence 不炸 | ⬜ 验收后按需 |
| **7** | （可选）You segment / 路径 chip | 与药丸一致 | ⬜ 按需 |

### 手工场景（对应步 5）

1. 开 thinking 的 agent，问一个会先想再答的问题 → 先见 Thinking，再见 Reply；类型稳定。  
2. 多 tool 回合：tool 前若有一句短说明 → 结束后仍是 Reply。  
3. 纯 tool 无正文 → 不出现空 Reply 把 thought 挤没。  
4. 刷新/重载 transcript → thought 与 reply 仍分开。  
5. 回归：图片 You 卡、@ 子任务、plan 面板。

---

## 6. 明确不做（本规范）

1. 引入完整 `ai` SDK / AI Elements 依赖树  
2. 首屏 Streamdown 重插件  
3. 重写整个 Clean 为 part 虚拟列表  
4. 与 Codeg 对齐的委派/导入/多端  
5. 用「更聪明的启发式」替代协议类型  

---

## 7. 成功标准

- 用户主观：**不再觉得「思考/正文被吞」**  
- 技术：`assistant_message` 类型在写入后 **只追加 duration，不改 type**  
- 产品：包体与交互仍克制；无新大型依赖  

---

## 8. 相关

- Marionette：`src/lib/acpTranscript.ts`、`SessionView.tsx`  
- 对照：`codeg-main/src/lib/adapters/ai-elements-adapter.ts`、`reasoning.tsx`、`content-parts-renderer.tsx`  
- 既有功能路线：`docs/superpowers/specs/2026-07-28-marionette-feature-roadmap.md`（主功能已齐；本文件是**体验与正确性**补丁）
