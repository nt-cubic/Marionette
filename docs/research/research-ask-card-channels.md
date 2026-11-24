# 问答卡通道矩阵（哪个 Agent 能问什么、能带什么）

问答卡（`AskQuestionCard`）只有两个来源：Grok 的私有方法 `x.ai/ask_user_question`，以及
ACP 的 `elicitation/create`（form）。每家适配器给 elicitation 喂的 schema 不一样，
「其他（替代答案）」和「补充（附注）」能不能用、走哪个字段，全由 schema 决定——
**不要按 agent 名字写死规则**。

实测/源码核对日：2026-11-22。

## 一、矩阵

| Agent | 通道 | 其他（替代答案） | 补充（附注） | 证据 |
|---|---|---|---|---|
| Grok | `x.ai/ask_user_question` | ✅ `answers[q] = 自定义串` | ✅ `annotations[q].{preview,notes}` | 真机探针（见 §二） |
| Codex | elicitation | ✅ `None of the above` + note，或主字段任意串 | ✅ `{q}_note`（`_meta.codex.role="user_note"`） | codex-acp `dist/index.js:41722-41903` |
| Claude Code | elicitation | ✅ `question_<n>_custom`（无 pick 时=答案） | ✅ 同一字段有 pick 时→`annotations[q].notes` | claude-agent-acp `dist/elicitation.js:85-288` |
| OpenCode | elicitation | ✅ `q0_custom` | ❌（只有一个自由文本槽） | `q0` oneOf + `_custom`；`cargo test opencode_question_form` |
| ZCode | elicitation | ✅ `q_<i>_other`（单选覆盖、多选追加） | ❌ | zcode-acp-server `dist/interaction/adapter.js:498-636` |
| DSH / DeepSeek | elicitation | ✅ `question_<n>__other`（单选覆盖、多选追加） | ❌ | deepseek-harness-acp `dist/server.js:2366-2475` |
| Antigravity | 私有 `session/request_permission` + `interaction_<hex>` | 未接（今天落在权限卡） | 未接 | `research-antigravity.md` §六；codeg 的 `try_bridge_antigravity_ask` |
| Pi | 只有 `session/request_permission` | — | — | pi-acp `dist/index.js` 无 elicitation/question |

## 二、Grok 真机探针（2026-11-22，本机）

裸 ACP NDJSON 探针：`grok --trust agent stdio` → `initialize` → `session/new` →
`session/session/prompt`（要求模型调 `ask_user_question` 问三题）→ 逐字段应答。
探针脚本与日志在 `%TEMP%\grok-ask-probe\`（不进仓库）。

请求形状（`x.ai/ask_user_question`，id=0）：

```json
{"questions":[
  {"question":"Which color do you prefer?","options":[{"label":"Red (Recommended)","description":"Red"},{"label":"Blue","description":"Blue"}],"multiSelect":false},
  {"question":"Name a color that is not in the list","options":[...],"multiSelect":false},
  {"question":"Which extras?","options":[{"label":"A (Recommended)"},{"label":"B"},{"label":"C"}],"multiSelect":true}
]}
```

应答（一次覆盖三种通道）：

```json
{"outcome":"accepted",
 "answers":{
   "Which color do you prefer?":"Blue",
   "Name a color that is not in the list":"Purple-NOT-AN-OPTION",
   "Which extras?":["A","C"]},
 "annotations":{"Which color do you prefer?":{"preview":"","notes":"NOTE-FROM-PROBE-1"}}}
```

模型下一轮原样复述：

```
- Blue
- NOTE-FROM-PROBE-1
- Purple-NOT-AN-OPTION
- A
- C
```

结论：

- **自定义答案**（不在 options 里的串）被接受，作为 `typed` 传给模型（Grok TUI 本身的
  `Other / Type your answer here` 也是这条）。
- **多选发数组**（`StringOrVec`）被接受；单元素数组也接受。旧实现把多选 `join(", ")`
  成一个字符串，标签本身含 `", "` 时会产生歧义——现在发数组。
- **`annotations[q] = {notes}` 是 Grok 的原生补充通道**，`notes` 原样进模型上下文。
  `preview` **可省略**（省略时模型不会看到空的 "selected preview:" 行），实现只发 `notes`。
- 自定义答案与补充可以同时给：`answers[q]` 是自定义串、`annotations[q].notes` 是补充。
- 探测环境提示：本次模型实际走引擎桥（`hub/codex/gpt-6.1-terra`），因为 `~/.grok`
  默认模型 `grok-4.6` 不在引擎目录里，先 404 再回落。与问答协议无关。

## 三、落地形态（三种）

```
A. 选项 + 其他                     B. 选项 + 其他 + 补充                 C. 只有选项
   OpenCode / ZCode / DSH            Grok / Codex(isOther) / Claude       严格 enum / Pi
   选项行                            选项行                               选项行
   ─────────                         ─────────
   其他 [替代答案]                   其他 [替代答案]
                                     补充 [可选：加一句说明]
```

- 单选再点一次取消；多选点选切换。
- 「其他」与选项互斥（单选、多选都一样）；「补充」是纯附加，不参与互斥。
- Codex 的 `None of the above`、ZCode 的 `__skip__` 是哨兵，不能渲染成普通选项
  （解析时就丢弃，`value_by_label` 里不留）。
- Codex 的 `_note` 只在 `question.isOther && options.length > 0` 时存在；`isOther` 也可从
  `prop._meta.codex.isOther` 直接读到。`_note` 撞名会生成 `_note1`/`_note2`，归属以
  `_meta.codex.questionId` 为准（后缀只是兜底）。
- **「其他」只写伴生字段，不写主字段**：同时写会让 Claude 把同一个串记成答案 + notes，
  多选还会被读成 `"x, x"`。
- OpenCode / ZCode / DSH 的单选「其他」是**覆盖**已选 pick（不是附注），UI 的互斥正好
  吻合；它们没有附注通道，所以不显示「补充」。

## 四、实现落点（2026-11-22）

- Rust：`elicitation.rs::parse_form_questions` 从 schema 算 `other_allowed` /
  `note_allowed`（`_meta.codex.isOther`、`_note` 伴生、Claude 的 `_custom`），
  哨兵过滤，`questions_for_ui` 把两个标志发给前端；单题表单用 `message` 兜住
  Claude 把题干放在 `message` 的情况。
- `elicitation.rs::build_questions_response`：`{question, selected, typed, note}` →
  按 Codex（sentinel + `_note`）/ Claude（主字段 + `_custom` 作 notes）/ 伴生字段
  （只写伴生）/ 纯文本（写主字段）分流。
- `acp.rs::build_ask_user_question_result`：`typed` 直接当答案；多选发数组；
  `note` 进 `annotations[q].notes`（不带空 `preview`）。
- 前端 `AskQuestionCard.tsx`：按 `other`/`note` 标志渲染三态；单选再点取消；
  「其他」与选项互斥；纯文本题只渲染一个「答案」输入框。

## 五、开放问题

- OpenCode 的 `q0_custom` 与主字段同时非空时，桥接层是把两端拼起来还是覆盖，未验证
  （bytecode，无源码级证据）。因此按「不做补充」处理；「其他」只写伴生字段。
- Antigravity 的原生问答桥未接（记录在 `research-antigravity.md` §七 #13）。
- 客户端 AIR 能力声明是扁平 `_meta["jetbrains.air"]`，而 codex-acp / claude 读嵌套
  `_meta.jetbrains.air`——我们始终按非 air 客户端走（`None of the above` 会出现），
  但那条 `sessionFailure` 声明实际上是空转。与问答卡无关，留待单独处理。
