# 07. 扩展借鉴目录（完整清单）

本文覆盖目前从 ego-lite 开源 harness、Agent Skill、native contract 文档、真实浏览器测试和 CitroLabs benchmark framework 中发现的剩余可借鉴项。

等级：

- **D 直接借**：规则、纯逻辑、测试思路可直接采用。
- **A 适配**：借契约或算法，改成 Rust/WebView2。
- **R 重写**：产品价值明确，但 transport、UI 或安全模型必须重做。
- **REF 只参考**：借思想，不复制实现。
- **NO 不借**：与产品边界或安全模型冲突。

## 1. Agent 行为与可观察性

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| 观察经济原则 | D | `browser_help` + MCP instructions：先 state/ready，必要时 snapshot，视觉问题才 screenshot；默认不同时拿多种观察 | P0 |
| 成功后立即停止 | D | 工具结果明确“目标状态已满足”；help 禁止重复确认同一结果 | P0 |
| 动作可见化 | R | 页内 closed Shadow DOM 光标/高亮/动作标签，3–6 个词，纯文本限长 | P1 |
| 当前 Agent 任务状态可见 | R | tab/页面显示 running、waiting_user、handoff、paused，不只在对话流里写 | P1 |
| 动作 receipt | A | 返回 dispatched action、immediate popup/dialog、当前 URL；不把 receipt 当最终业务成功 | P1 |
| 未处理新页面汇总 | A | round/batch 结束时列出未观察 popup/new tab，附 opener 与 URL | P2 |
| Node/Page 上下文错误提示 | A | 若未来有 JS workflow，针对 `document/window/localStorage` 误用给出“请在 page.evaluate 中运行”的提示 | P2 |
| 不从新 tab 逃避卡页 | D | help 和错误指引规定：先检查/恢复当前 tab，不自动新开 tab 假装恢复 | P0 |
| 不做无意义盲重试 | D | 错误码返回可重试性和下一步观察建议，禁止立刻换坐标/raw CDP | P0 |
| 动作审计摘要 | A | 记录 read/click/type/select/refuse/fail 的 origin、tab、request、结果 | P1 |
| 动作 label 与审计共用语义 | R | 同一个用户可读 summary 同时用于页内光标和活动记录，不能两套漂移 | P1 |

### 观察经济的具体规则

建议直接进入 browser Skill/help：

1. 先用最便宜的状态回答“页面是否变了”。
2. 需要找可操作元素时才 snapshot。
3. 需要视觉判断、布局验收或 DOM 语义不足时才 screenshot。
4. 一次观察足够计划多步时，连续完成动作后再观察。
5. 页面已经明显达到目标时停止，不重复 screenshot/snapshot 确认。
6. 动作失败先看当前状态，再决定重试、等待还是向用户求助。

## 2. 控制权、暂停与生命周期

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| Hard stop | A | 用户接管、权限弹窗、handoff、人工登录时 tab/task 进入停止态；工具返回不可绕过的 guidance | P0 |
| 全局 native 操作门 | A | ego 的 `NativeOperationGate` 对应现有 `CdpLease`；禁止多个全局选择/ CDP 操作交错 | P0 |
| 保守 ownership | D | `unknown` 一律视为用户所有；Agent 不得关闭、导航或自动 adopt 未经确认的页面 | P0 |
| 稳定 Page label | A | 叠加在现有 chosenId 上，不新建第三套 tab identity | P1 |
| adopt/release/claim | A | Agent 可把 unmanaged tab 纳入任务；release 只交还，不关闭；claim 清 ownership/grant | P1 |
| finish keep/close 规则 | A | session/task 结束支持 keep all / keep labels / close managed；保护 pinned/user/waiting tab | P1 |
| Page budget | A | 每 task/session 限制 managed tabs 数，触顶报错并建议复用/释放 | P2 |
| 未处理 popup/new tab 发现 | A | 新页面先以 unmanaged notice 出现，不自动归 Agent | P2 |
| 用户控制边界后不自动 adopt | D | 用户接管期间出现的新 tab 保持 unknown，恢复后由 Agent 明确 adopt | P0 |

### Hard stop 的边界

Hard stop 是“不要继续做”的强行为契约，但不是拥有 shell 权限的恶意 Agent 的安全沙箱。Marionette 可做到：

- browser tools 全部返回 hard-stop；
- 禁止自动新 tab 绕过；
- UI 显示等待人工；
- 撤销/暂停 grant；
- 必要时暂停整个 Agent 回合。

不能仅靠工具错误假定 Agent 绝不会尝试其它工具。

## 3. Snapshot、ref 与动作语义

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| 同一 snapshot 连续动作 | A | ref lease 独立于 action counter；只在导航/断开/替换时失效 | P0 |
| 稳定 ref identity | A | frame/document/node identity ledger；第一版只放进程内 | P1 |
| stable locator 诚实性 | D/A | 只有唯一且指向同一 node 才输出；`unstable/ambiguous` 直接省略 | P1 |
| Snapshot compaction | D/A | 删空白、空 container、单子 container；保留语义节点与顺序 | P1 |
| Snapshot scope | A | viewport/full page/subtree；subtree 要 engine 支持后再开 | P2 |
| 自动滚入可操作区域 | A | click/fill/hover 前先 wheel 滚动，失败再程序化 scroll | P1 |
| Actionability 分类 | A | not found / hidden / disabled / pointer blocked / detached 分开 | P1 |
| fill 生效校验 | A | 不信“事件已派发”，读取控件值并允许格式转换等价 | P1 |
| click 真 hit target | A | 点击点必须命中目标或可接受包装元素；不能只按坐标盲点 | P1 |
| open shadow DOM | REF | CSS/选择器尽量穿透开放 shadow root；closed root 不承诺 | P1 |
| iframe / OOPIF | A/R | 先 same-origin，后 per-frame session；遵守 single-owner CDP scheduler | P2 |
| raw CDP invalidates refs | D | 任何 raw eval/CDP 后要求新 snapshot，除非有明确映射 | P0 |
| targetId 不跨轮次持久化 | D | target id 是内部身份；对外继续用 Marionette tab id / Page label | P0 |

### Action receipt 与业务结果分开

Receipt 只说明“动作派发以及立即观察到的 popup/dialog”，不代表：

- 表单提交成功；
- 服务端接受了请求；
- 页面跳转完成；
- 业务目标已达成。

业务成功必须来自后续 page state、URL、DOM 状态、用户可见结果或截图验收。

## 4. Wait、事件、弹窗与媒体

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| waitForSelector | A | attached/visible/hidden/detached + deadline | P1 |
| waitForLoadState | A | domcontentloaded/load；networkidle 后置 | P1 |
| waitForURL | A | exact/glob/regex/predicate 的缩小版 | P2 |
| waitForEvent | A | popup/download/fileChooser 先武装 waiter 再执行触发动作 | P2 |
| page events ring | A | console/dialog/popup/download 统一缓冲、上限和 missing 计数 | P1 |
| JS dialog | A | accept/dismiss；page modal 打开时快速失败 renderer-bound CDP | P1 |
| popup receipt/waiter | R | 保留 opener 归属；延迟 popup 也归到来源 task | P2 |
| download contract | R | URL、suggestedFilename、temp path、saveAs、failure/cancel/delete | P2 |
| file chooser contract | R | multiple、setFiles、结果、dialog；限制路径范围 | P2 |
| clipboard transaction | A | 临时写剪贴板后必须恢复；全局串行；恢复失败要明确说 paste 已完成 | P2 |
| pending navigation | A | 点击后导航未提交不能永久卡住；超时后说明旧/新 document 状态 | P1 |
| page unresponsive | A | 区分 renderer 无响应、dialog 阻塞和普通超时 | P1 |
| network idle | A | 按 Page/OOPIF 独立请求状态，不串到其它 tab | P2 |
| media/canvas fallback | REF | DOM 无意义时 screenshot + mouse/keyboard；canvas 可用页面内 toDataURL 兜底 | P2 |

### Dialog 红线

- 页面 `confirm/prompt` 永不自动接受。
- 普通权限卡与 page-authored JS dialog 不能共用一个“allow”语义。
- dialog 打开时，只允许浏览器进程级 CDP 和 `Page.handleJavaScriptDialog`，其它 renderer-bound 调用快速返回 dialog-blocked。
- dialog 结束后必须清理 pending 状态，否则后续工具会永久误报。

## 5. 运行时卫生与失败恢复

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| 单 CDP 请求超时 | A | 所有 CDP 调用有 per-method deadline，错误带 method/tab | P1 |
| session setup 短等待 | A | session/world 准备不能吃掉整个动作预算 | P1 |
| session 丢失恢复 | A | context destroyed / target closed 分类；必要时重装 world | P1 |
| event buffer 上限 | A | 每个 tab/event channel 有 ring 和 missing 计数 | P1 |
| disposed runtime 清回调 | A | tab close/app exit 拒绝 pending 请求、移除 binding、清 subscriber | P1 |
| hard-stop output sink | A | hard stop 后丢弃半截“成功”输出，只显示一次权威停止指引 | P1 |
| syntax/step diagnostics | A | batch/expression 错误给 stepIndex、op、字段路径和源码行 | P1 |
| update notice | A | 版本升级提示 best-effort，不影响任务成功；升级后要求重读 help | P1 |
| stale API/skill guard | A | API 版本不匹配时 hard stop，提示重新拉取 help/Skill | P1 |
| CDP 事件不得嵌套发 CDP | D | 继续现有 zero-CDP event handler 红线 | P0 |

### 输出完整性

ego-lite 在 hard stop 时会把当前 round 的业务输出全部丢弃，只保留一次停止指引。Marionette 的对应规则应是：

- 页面已暂停/用户接管时，不再向对话流追加“完成了…”的中途成功消息；
- 已产生的动作审计仍可保留；
- Agent 最终只能看到权威 hard-stop reason 与恢复方式；
- 恢复后开启新的明确阶段，不接着伪装旧回合成功。

## 6. 帮助、Skill 与版本漂移

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| API schema 单一真源 | D/A | 工具 schema、help、Skill、示例从同一表生成 | P0 |
| 按需 help | D | `browser_help(topic)`，避免 tools/list 无限膨胀 | P0 |
| 版本化指南 | A | help 返回 Marionette 版本/capability bits；未启用功能不出现 | P1 |
| 可选更新提示 | A | 有新版本时在任务结束后提示，不中断任务 | P2 |
| stale Skill 检测 | A | Skill 使用不存在 API 时明确要求重新读取，而不是继续猜 | P1 |
| 平台差异说明 | D | Windows 路径、快捷键、剪贴板和下载行为写入本平台指南 | P1 |

## 7. 站点经验包与学习系统

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| 站点 manifest | A | 域名、说明、工具列表、输入/返回 schema、版本 | P2 |
| 纯文本笔记 + 可执行工具分离 | D | notes 只当资料；tools 要显式验证后才能运行 | P2 |
| 域名模式校验 | D | 只允许 `example.com` / `*.example.com`，禁止 URL、路径和裸 `*` | P2 |
| 工具路径安全 | D | 相对路径、拒绝 `..`、反斜杠和绝对路径 | P2 |
| args/returns schema | D | 每个站点工具必须声明输入输出 schema | P2 |
| 站点工具来源与版本 | A | hash/version/source/最近验证时间；过期先提示 | P2 |
| 秘密排除 | D | 禁止 Cookie/token/密码/个人数据进入经验包 | P0 |
| 项目本地优先 | R | 先支持当前项目的 localhost QA recipe，再做全局站点包 | P2 |

### 建议的 Marionette 形态

不要把 ego-lite 的 Node 文件执行器直接搬进便携版。优先使用现有 Skill/项目上下文：

```text
project/.marionette/browser-recipes/
  manifest.json
  notes.md
  recipes/<name>.json
  fixtures/<name>.html
```

Recipe 先定义为受限 typed steps（与 `browser_batch` 同构），需要更复杂逻辑时再升级执行器。

## 8. Benchmark 与评测体系

来源：<https://github.com/citrolabs/ego-browser-benchmark-framework>（MIT，public beta）。

| 项目 | 等级 | Marionette 做法 | 阶段 |
|---|---|---|---|
| Stage A execute / Stage B judge / Stage C report | REF | 执行、判定、报告独立产物，重新 judge 不重跑任务 | P2 |
| 固定控制变量 | D | 同 model/tool/skill/judge/limits/concurrency 才能横向比较 | P2 |
| session JSONL 事实源 | D | turns/tokens/cost/tools/duration 从原始 session 派生 | P2 |
| tasks.jsonl 只存状态与 artifact 引用 | D | 不重复复制派生指标 | P2 |
| screenshot evidence | D | 每任务截图去重、保留 frame、报告可审计 | P2 |
| deterministic local sites | D/A | 先建 Marionette localhost fixture site，CI 不依赖线上网站 | P1 |
| task resources preflight | D | attachment/local-site 缺失在首轮模型调用前失败 | P1 |
| screenshot SHA-256 dedup | D | frame 去重，避免长任务截图膨胀 | P2 |
| AgentSurface / EngineControl 分离 | A | 被测 Agent 工具面与截图/清理控制面分离，避免评测污染 | P2 |
| close audit | D | teardown 的 ok 是真实关闭结果，非“未抛异常” | P2 |
| prompt injection fixture | D | 表单/附件里故意放指令，单独考核 Agent 是否抵抗 | P1 |
| judge model 与 image concurrency 分离 | REF | 图像证据可限流，避免 judge 成瓶颈 | P2 |
| live-site failure accounting | D | 站点改版/反爬与 Agent 失败分开归类 | P2 |
| external dataset license review | D | 外部 dataset 的文本不自动继承 benchmark repo MIT | P2 |

### 不建议整套接入的原因

- 默认 `ego-browser` adapter 依赖 ego-lite；
- README 标注默认运行环境偏 macOS；
- Python 3.13、uv、pnpm 和大量线上任务不适合直接进 Marionette CI；
- 当前目标是产品回归，不是通用模型排行榜。

### 建议借的最小版本

1. 一个 localhost fixture server；
2. 10–20 个固定浏览器任务；
3. 每任务记录 session JSONL、最终状态、截图 hash 和工具统计；
4. rubric/judge 与执行分目录；
5. 报告按 task 重算，不覆盖原始证据。

## 9. 额外发现但没有列入首版的机制

| 项目 | 判断 | 原因 |
|---|---|---|
| 原生 `egoAutomated` CDP domain / 隐藏自动化痕迹 | 不借 | 与普通 WebView2 无关，接近站点检测规避 |
| site stealth / anti-bot 专项优化 | 不借 | 当前产品是开发工作区，不做通用抓取或反检测 |
| 真实 Chrome profile 迁移 | 不借 | 已明确超出 Marionette Browser 边界 |
| 完整 Node heredoc 执行 | 暂缓 | 便携版不保证 Node；typed batch 先覆盖主路径 |
| 任意 `page.evaluate` / raw CDP | 暂缓 | 调试价值高，但权限和审计面大，必须独立产品决策 |
| 完整 TaskSpace 用户 UI | 不借 | 会与 session/project/split workspace 形成第三套身份 |
| 整个 benchmark live-web 数据集 | 不直接借 | 许可证、稳定性、账号与反爬成本都高 |

## 10. 映射到现有 M0–M5 路线

| 里程碑 | 追加项 |
|---|---|
| M0 | 观察经济、成功后停止、不从新 tab 逃跑、保守 ownership、raw CDP 失效 ref、NativeOperationGate 红线 |
| M1 | 动作可见化、task 状态、动作审计、browser_help、平台指南、版本化 help |
| M2 | typed batch、action receipts、waits、dialog、pending navigation、step diagnostics |
| M3 | Page label/adopt/release/finish、Page budget、page events ring、hard-stop output sink、clipboard transaction |
| M4 | unhandled page notices、popup/download/file chooser、same-origin iframe、OOPIF 实验 |
| M5 | project-local recipes、站点 manifest、benchmark fixture/judge/report、prompt-injection regression |

## 11. 最终优先级

### P0：立刻做

- 观察经济与成功后停止；
- browser_help 单一真源；
- hard stop 与不得新 tab 绕过；
- 保守 ownership；
- same-snapshot 连续动作；
- NativeOperationGate/CdpLease 红线。

### P1：体验提升最大

- 动作可见化与审计；
- typed batch；
- waits/dialog/actionability；
- Page label/adopt/release/finish；
- hard-stop output sink；
- local fixture benchmark。

### P2：复杂能力

- popup/download/file chooser；
- page events/runtime caps；
- same-origin iframe/OOPIF；
- site recipes；
- full benchmark adapters。

## 12. 结论

当前已经没有明显遗漏的“可公开借代码”区域。下一层要继续增加收益，重点不再是继续找 ego-lite 文件，而是把本文已列行为变成 Marionette 自己的契约、测试和可观测指标。
