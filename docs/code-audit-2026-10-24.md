# 全库代码摸底报告（2026-10-24）

> 一次性全面体检的存档记录。方法：约 10 万行第一方代码切成 10 个子系统块，10 个 GLM-5.3 只读子代理
> 并行评审（grep 先行、限读片段、限工具调用；其中 1 个因 Zhipu 限流重跑成功），主代理对每份报告的
> 关键 file:line 论断逐条抽查——抽查 20+ 处全部属实，两处措辞修正、一处文件存在性修正
> （见文末「方法与抽查记录」）。分块原始报告约 4 万字，曾存 `fortest/code-audit-2026-10-24.md`
> （gitignored 暂存区，可能被清理）；本文件自包含，不依赖它。
>
> **行号基准 = 提交 `75b952c`（2026-10-24 01:52）。** 其后 `d9c7edf`/`95c09cd`/`abceaa6`/`51ddcc0` 已致漂移：
> app.css 9564→10678、Composer.tsx 3661→3717、MarkdownBody.tsx 310→371（urlTransform 267→328）、
> acpTranscript.ts 1859→1885（`[...current]` 复制点 1461/1470/1515→1493/1505/1585）、App.tsx 8050→8125、
> acp.rs 7074→7082、commands.rs ≈+37（npm_install_global 531-533→570）。动手前先按内容 grep 定位，勿直接跳行号。
>
> **状态约定：修复后在对应条目前加 ✅ 并注明提交号；条目全部处理完后本文件挪 `archive/`。**

## 范围与底数

| 部分 | 规模 | 主要巨型文件 |
|---|---|---|
| 前端 `src/` | ≈ 5.0 万行 | `app.css` 9564、`App.tsx` 8050（单组件 ≈7200 行）、`Composer.tsx` 3661 |
| 后端 `src-tauri/src/` | ≈ 4.8 万行 | `acp.rs` 7074、`model_bridge.rs` 4690、`engine.rs` 3324、`telegram_remote.rs` 4639（含测试） |
| `scripts/` + `tools/` | ≈ 0.65 万行 / 26 个脚本 | — |

## 总体结论

**质量高于同规模项目平均水平。** 事故驱动的注释文化（每次修复留根因）、`panic=abort` 下生产路径
几乎零 `unwrap/expect`（多份报告独立确认）、密钥 DPAPI 加密、解析器守卫式跳过、转录虚拟化真实落地、
拖拽全程 rAF——这些是好团队才有的特征。真正的债务集中在四个系统性主题：

1. **一个确凿的 P0**：Telegram bot token 会明文落进 `dev.log`（代码 + ureq 源码 + 日志通路三方定案）。
2. **热路径性能债**：「事件洪泛→假死」的根因只被 50ms 批处理降级，lib/组件层的 O(n) 结构还在；
   Rust 侧全局锁跨磁盘 I/O、每行日志 4 次系统调用、多处子进程等待无超时。
3. **存储与进程的鲁棒性缺口**：非原子写、无 fsync、坏配置整体替换无留档、桥写无进程内互斥。
4. **工程基建空白**：无 CI、无 ESLint/Prettier、无 `.editorconfig`、无单测框架
   （第一手确认；质量保障全靠 `scripts/` 下 26 个即席脚本）。

---

## 🔴 P0 — 立即处理

### Telegram bot token 明文落 `dev.log`（两处泄漏点，同一根因）

- **点 1** `src-tauri/src/telegram_remote.rs:2184`：poller 对 getUpdates 响应直接
  `.map_err(|e| e.to_string())`。
- **点 2** `telegram_remote.rs:3257`：`tg_call` 对 `ureq::Error::Status` 做了彻底清洗
  （3237-3255），但 transport 错误走 `other => format!("{method}: {other}")` 整体格式化。
- **泄漏机制**：Cargo 锁定的 ureq 2.12.1，其 `Error` 的 Display 会把完整请求 URL 拼进错误文本
  （本机 registry 源码 `error.rs:210-240` 亲验），URL 里就是 `bot<TOKEN>`。错误串经
  `inner.last_error`（≈2226）→ `log("warn", …, Some(&error))`（2229-2236）落 `dev.log`。
- **触发条件**：token 被吊销（401）、断网等日常故障即可——而 AGENTS.md 的排障流程第一步就是让
  用户看/贴 dev.log，路径非常现实。直接违反模块自己在 `telegram_remote.rs:3225` 的声明
  （「token 绝不进错误文本」）与 CURRENT.md 的安全底线。
- **修法**：两处错误改走 `tg_call` Status 分支同款清洗——只留方法名 + `ErrorKind` 类别，
  或 `replace(token, "***")` 后再入日志。

---

## 🟠 P1 — 显著问题（按主题分组，关键行均已亲验）

### A. 前端热路径性能 —— 「事件洪泛→假死」的结构性根源

App 层的 50ms 批处理（`ACP_EVENT_FLUSH_MS = 50`，批内逐事件 try/catch 隔离）只是降级；
lib/组件层单次成本仍随数据量线性/平方放大，流式 20-50Hz 时被乘出来。

| 位置 | 问题 | 建议 |
|---|---|---|
| `src/lib/acpTranscript.ts:1461`（另 1470/1515/1629/1421） | 每个 assistant token 都 `[...current]` 全量复制共享 rail，长 rail + 高频 token = O(n·m) 复制洪泛 | rail 按会话分桶或游标式原位更新 + 结构共享 |
| `src/lib/acpTranscript.ts:1724`（同型 1775） | `coalesceAdjacentThoughts` 对每个事件反向扫 `out` 找前驱；每 flush 调用则长 rail 最坏平方级 | 维护 lastSameSessionIdx 增量定位，一遍完成 |
| `src/lib/acpTranscript.ts:1581` | 每个 tool 分块对整条 rail `findIndex` 找 toolCallId | 每会话维护 toolCallId→index 的 Map |
| `src/lib/turnDelivery.ts:7` | 注释自认 "derived on every render"：`buildTurnDeliveries` 每次渲染多趟全量扫描 events | 按 (events 引用, sessionId, isRunning) memoize |
| `src/lib/useVirtualWindow.ts:58` | offsets 前缀和随 version 全量重建；流式期间每帧 O(n) 重算，万行级列表每帧重算 | 增量更新尾部 offset 或 Fenwick 树 |
| `src/app/App.tsx`（全文件 0 个 `memo(`，≈70 useState / 41 useEffect / 74 ref） | 单组件 ≈7200 行，任一 state 变化整树重渲；流式期间 `eventsTick` 每 50ms bump | 按文末拆分方案拆事件管线/面板/提示卡 |
| `App.tsx:7624-7743` | `ProjectShelf`/`SessionTabs` 收到大量内联箭头回调，流式时侧栏+标签栏持续重渲 | 回调 useCallback + 子组件 React.memo |
| `App.tsx:984, 2939` | `reportStreamInterests` 每次 flush 全量 `JSON.stringify` 比较，约 20Hz | 逐 key 浅比较或脏标记 |
| `App.tsx:5823-5835, 5868` | Esc/Ctrl+1..9 效果依赖随流式标题更新反复变化 → 键盘监听反复卸载重挂 | handler 存 ref，依赖收敛为 `[]` |
| `App.tsx:3154-3176` | 60s 自动挂起 interval 依赖三个 prompt state，提示出现/消失即重置计时窗口 | prompt 快照存 ref |
| `App.tsx:7157-7160` | `resolvePathGrant` eslint-disable 掉 `performSend`，闭包可能过期 | `performSend` 存 ref |
| `src/components/Composer.tsx:674-676` | 每击键同步写 localStorage 草稿，无防抖 | 300-500ms 防抖 + 卸载 flush |
| `Composer.tsx:1623-1644` | 每击键对整份 `sessionEvents` 跑建议启发式，且同步读 localStorage flag | flag 结果缓存；`suggestFor` 只随 round 重算 |
| `src/components/SessionView.tsx:2066, 2151` | 子代理卡在渲染路径对全量 events `filter`，且逐行渲染子流 | useMemo 预构建 Map；子流只渲染尾部若干行 |
| `SessionView.tsx:2864-2899` | 行节点闭包每渲染新建、行级卡片无 memo，流式期间全窗口 reconcile | 抽 EventRow memo 组件 |
| `src/components/ProjectShelf.tsx:940-942, 964` | 每次渲染（含拖拽每个 pointermove）对每项目未 memo 的 `filter+sort` | useMemo 按 projectId 预聚合；hint rAF 节流 |
| `src/components/UnifiedDiffView.tsx:58-111` | diff 无行数上限：单卡可达数千行 × 6-7 span → 数万 DOM；ClippedBody 只裁视口不裁 DOM | parse 后截断（如前 1000 行 + 省略提示） |
| `src/components/MarkdownBody.tsx:88` | 模块级图片 base64 缓存无上限、无 LRU，长会话内存单调涨 | 条目上限（如 50 张 LRU）或随会话清理 |

### B. Rust 热路径 / 假死面

| 位置 | 问题 | 建议 |
|---|---|---|
| `src-tauri/src/acp.rs:398-901` | `start()` 持全局 sessions 锁跨 PATH/auth 磁盘 I/O 与 spawn，期间其它会话 start/stop/cancel/process 全部排队 | 锁内只做查重与插入，环境准备挪锁外 |
| `src-tauri/src/debug_log.rs:69-106` | 每条日志在全局锁下 4 次系统调用（create_dir_all/metadata/open/write）；慢盘/杀软时串行化全部线程 | 常驻写线程 + channel，句柄复用，轮转移出关键路径 |
| `src-tauri/src/storage.rs:919-934` | 转录每 tick 全量重写整个 JSONL（长会话 O(n²) IO），且 `fs::write` 非原子无 fsync | 追加写或分段；至少 temp+rename；tick 节流 |
| `src-tauri/src/git_service.rs:808-819` | `run_git` 的 `cmd.output()` 无超时（全文件零 timeout），且缺 `GIT_TERMINAL_PROMPT=0` 等防护（334-357 的 snapshot 变体反而有） | 包 wait_timeout；补 env 防护 |
| `src-tauri/src/commands.rs:531-533` | `npm_install_global` 裸 `command.output()` 无超时；npm 挂死永久占住线程。`agent_update.rs:49-53` 自己注释过这个坑并为 `--version` 写了 `output_with_timeout`，此处未复用 | npm install 也走 `output_with_timeout` |
| `src-tauri/src/agent_update.rs:53-65` | `output_with_timeout` 超时后只丢结果：阻塞线程与卡死 CLI 子进程永不回收，随每次 preflight 累积僵尸 | 超时分支 `child.kill()`，或 Job Object 绑定 |
| `src-tauri/src/telegram_remote.rs:2342, 2373-2389, 2413` | poller 线程内同步 `send_message`（阻塞 HTTP），每条未知命令把长轮询串行卡住；已有专用 sender 线程（1739）却未走 | 回复全部改 `enqueue_send` |
| `src-tauri/src/model_bridge.rs:1549-1560` | bindings / zcode 的 load→merge→save 序列无进程内互斥：两会话并发 spawn 触发两次外科手术，后写覆盖前写 | 桥写入入口加 Mutex 串行化 read-modify-write |

### C. 安全与信任边界

| 位置 | 问题 | 建议 |
|---|---|---|
| `src/components/MarkdownBody.tsx:267`（配合 29-36） | 自定义 `urlTransform` 替换默认净化器：`javascript:`/`data:text/html` 原样进 `<a href>`；点击虽被 MdLink 拦截，净化层已被移除，任何绕过路径即成 XSS 面 | 协议白名单：仅 http(s)/`file:`/盘符，其余返回 `#` |
| `src-tauri/tauri.conf.json:28` | `"csp": null`，WebView 无内容策略 | 显式 CSP（`default-src 'self'` 按需放开） |
| `src-tauri/tauri.conf.json:29-31` | assetProtocol 启用且 scope `["**"]`，整个文件系统经 asset: 可读 | 收窄到会话/图片目录白名单 |
| `src-tauri/src/commands.rs:2155-2240` | `read_image_data_url` 接受任意绝对路径无根约束，≤12MB 任意本地文件可 base64 进模型上下文；`resolve_image_path` 后缀模糊匹配可能命中别的文件 | 限定 session cwd + `~/.marionette/` 白名单 |

### D. 数据安全 / 存储耐久

| 位置 | 问题 | 建议 |
|---|---|---|
| `src-tauri/src/storage.rs:1126-1139`（配合 45-52, 35） | `write_sessions`/`write_projects` 非原子 `fs::write`：断电截断 `projects.json` 后 `from_global_dir` 直接 Err，无备份无修复 → 存储整体变砖 | `.tmp` + rename（参考 `memory.rs:130-141` 的仓库最佳实践）；启动损坏从 `.bak` 恢复 |
| `src-tauri/src/model_bridge.rs:801-805` | zcode 合并时 `from_str(...).ok()` 失败即 `unwrap_or_default()`：用户 provider_config.json 半写坏后被静默整体替换、自建 provider 丢失且无留档——与 credentials.rs:289-308 的留档纪律不一致 | 整体替换前先 rename 留档（复用 archive_corrupt 模式） |

### E. 工程卫生与基建

| 位置 | 问题 | 建议 |
|---|---|---|
| `package.json:6-13` | 无 `lint`、无 `test`、无独立 `typecheck`（tsc 藏在 build 里） | 补三个脚本 |
| `src/lib/api.ts`（44+ 处）、`detachedWindow.ts`（27 处） | 裸 `catch {}` 静默兜底返回默认值，无日志通道（[核] 非字面空块，问题实质成立） | 接入统一 debug 日志，区分「可选能力」与「真失败」 |
| `tsconfig.json:10` | 有 `strict` 但缺 `noUncheckedIndexedAccess`、`noUnusedLocals/Parameters` | 补齐并修复暴露问题 |
| `src/styles/app.css:449,465,2639,8176,8887,9209; tokens.css:30` | 同一「危险红」四套并存（#e25555/#e35d6a/--danger #ef4444/--red #ef6c73），fallback 与 token 值互相矛盾 | 统一 token，删内联 fallback |
| `src-tauri/src/parsers/mod.rs:116-151` | 每源独立线程 + `join()` 捕 panic 的隔离设计在 release（`panic=abort`）失效：首个解析器 panic 直接杀进程，「单源失败不传染」仅 debug 成立 | 保持无 panic 约定 + CI 加 debug 构建跑坏输入测试兑现承诺 |
| `scripts/`（18 个文件） | 逐字相同的 `check()` 计数 harness 复制 18 份；26 个脚本无聚合 runner | 抽 `scripts/lib/harness.mts` + 聚合 `npm run verify` |
| 仓库根 `build-now.ps1:3` | 第二入口 + 硬编码 `Set-Location 'D:\Projects\Marionette'`；另有本地残留 `zdiag.ps1`、`zdiag-out.txt`、`zdiag-zcode.ps1`、`zdiag-zcode-out.txt`、`preview-transcript.local.jsonl`、`.tmp-build.log`（[核] t1 曾误报 not found 且写错名；`2e00805` 已把前五类 gitignore，文件本体仍在） | 挪 `tools/`（内部改 `$PSScriptRoot`）；残留删除 |
| `tools/build-portable.bat:166-173` | UPX 压缩失败仅 `[warn]` 继续；`REQUIRE_UPX=1` 只拦「未安装」不拦「压缩失败」→ 7MB 未压缩产物可能静默发布 | REQUIRE_UPX=1 时 UPX 失败 goto fail |
| `tools/build-portable.bat:14` | 引擎种子默认拉 GitHub latest stable 不锁 tag，构建不可复现 | 默认读 `third_party/magpie/VERSION`，显式 MAGPIE_TAG 才升级 |
| `tools/start-marionette-quick.bat:26` | `call npm install` 后不查 errorlevel 就起 Vite/exe | 补 `if errorlevel 1 exit /b 1` |
| `tools/rebuild-quick.bat:16` | `taskkill /IM marionette.exe /F` 按映像名全杀，误伤多开实例 | 按 PID/窗口标题精确终止 |

---

## 🟡 P2 — 摘要（按子系统，位置均经报告给出、抽样核对属实）

**前端 lib / App / Composer**：`acpTranscript.ts:118/324/529/1067` JSON.parse 失败静默回退无痕迹；
`acpTranscript.ts:1400` `findLastThoughtInTurn` 正向全量扫（应从尾反向）；`sessionSuspend.ts:93`
数组 `includes` 应传 Set；`sessionOrder.ts:25` 坏 pinnedAt 返 0 应返 null；`sessionFork.ts:45`
`as` 断言可造非法联合变体；`useVirtualWindow.ts:121` rAF 句柄无卸载清理；`bridge.ts:48` 无 in-flight
去重可致重试风暴；`sessionBus.ts:60` unlisten 竞态泄漏 Tauri listener；`App.tsx:1356/1361/1337/7127`
静默 catch；`App.tsx:2911-2929` flush 批内不分片，重放/追赶仍可长阻塞；`App.tsx:3967-3970`
`void eventsTick[]` 依赖 hack；`App.tsx:991-998` 全会话共享一个 eventsTick 互相放大；
`Composer.tsx:755/764` + `App.tsx:3900` localStorage 空 catch；`slashCommands.ts:138-140`
冗余分支；`Composer.tsx:1618` 高频内联函数。

**前端面板组件**：`SessionView.tsx:1216` 每行 new 一个 ResizeObserver（满载 ~72 个）；
`SessionView.tsx:1909-1923` observer churn（totalSize 在依赖里反复 disconnect/re-observe）；
`ProjectShelf.tsx:687-699` pointermove 同步 elementFromPoint 无节流；`ProjectShelf.tsx:937`
项目列表未虚拟化；`ContextPanel.tsx:262`、`MemorySection.tsx:36` 缺 memo；`MemorySection.tsx:62-64`
探测失败压成空数组与注释意图相悖；`SessionView.tsx:1570-1571` 渲染体内改 ref（并发模式未定义行为）；
`SessionView.tsx:513-532` 每渲染重建 className/内联 handler。

**Markdown / 对话框**：四个对话框各自实现 backdrop/Esc/aria 且行为不一致（ConfirmDialog 有 Esc+
默认焦点取消，CredentialsDialog 仅 backdrop，ImageAnnotator 都没有）→ 抽公共 Dialog 壳；
`CredentialsDialog.tsx:147-156` `handleEngineEnable` 无 catch，引擎下载/启动失败无提示（unhandled
rejection）；`ProviderConfigDialog.tsx:46-48` 加载失败静默空列表；`CredentialsDialog.tsx:95-97`
吞错误详情；`PlanApprovalCard.tsx:58-59` 丢错误对象；`AskQuestionCard.tsx:70-73` 依赖缺
`prompt.questions`；`MarkdownBody.tsx:125-129` `data:`/`blob:` img src（可接受，随链接白名单一并收紧）。

**样式 / 前端配置**：mode-tone 调色板完整重复 4 遍（app.css:4350/4449/7167/7313）应收敛为 token；
z-index 三套量级混用（个位数/1200/9990+）无分层文档；`app.css:977-978` 对自身规则 `!important`
（特异性失控）；全文件 0 个 `@media`，动画无 `prefers-reduced-motion` 降级；
`recentModels.ts:88,113`、`desktopNotify.ts:92-93` 迁移读旧 key 不删除；localStorage key 三套前缀
无全局清单；`delegate.ts:118-120` 模型 0 命中静默接受任意 modelId；`acpMeta.ts:40` detached 多窗口
合成 id 可撞；`vite.config.ts` 无 manualChunks；`index.html` 与 `theme.ts` 退役主题映射双写。

**Rust ACP 核心 / 入口**：`acp.rs:4037-4066`（另 4117/4202/4256/4319）每个 permission/question/
elicitation/plan 起一个睡全时长的线程，应单定时器 + 到期队列；`acp.rs:3553-3556` stdin 写线程退出
被误报成 "timed out"；`acp.rs:3773-3777` 迟到应答静默丢弃无痕迹；`main.rs:405-407`
`StorageService::new().expect` 在 panic=abort + windows_subsystem 下是无对话框静默退出（与
data_dir_gate 兜底哲学相悖）；`main.rs:388-400` `Destroyed` 安全网不跑 `stop_all` → agent 孤儿；
`debug_log.rs:74-78` 轮转只留一代且每条 append 都重新 metadata；`session_manager.rs:19,37`
map 值从不读取（假状态，应 HashSet）；`app_paths.rs:31-39` 目录迁移发生在单实例锁之前，双开竞态
可数据分叉；`acp.rs:1219-1229` `stop_all` 虚报成功数；`main.rs:541-542` `run().expect` 同为静默 abort。

**Rust 模型 / 引擎 / 凭据**：`engine.rs:685-688` `replace_engine_exe` 两步间镜像缺位，失败可恢复但
状态倒退（应自动触发工厂恢复）；`engine.rs:1746-1761` idle_watchdog `kill()` 后不 `wait()` 回收，
且重启端口变更使外部 baseUrl 失效；`model_bridge.rs:1351`、`http_client.rs:36-38` HTTP 响应无大小
上限（该请求带 Bearer key）；三处原子写 rename 前无 `sync_all`（掉电可落空文件）；
`model_bridge.rs:1968,1980`、`model_sources.rs:354` `last_mut().unwrap()` 依赖隐式不变量；
`http_client.rs:11-16` `OnceLock<Result>` 初始化失败永久粘滞；`engine.rs:504-532` 流式下载有 sha256
但无体积上限。

**Rust 命令面 / 终端 / Telegram / 用量**：`telegram_remote.rs:1705,1727,1401+1843-1850` outbound
无界 mpsc，长故障期队列无上界（应 sync_channel + 丢旧合并 Dashboard）；`telegram_remote.rs:2201-2209`
getUpdates ok:false 分支固定 15s 重试无退避增长（[核] 传输/HTTP 错误走 Err 分支，本就有 1s→60s
指数退避，2243；401/403 的真缺口是永不停用无限重试，应停用并提示）；`terminal_runtime.rs:375-383`
`clear_session` 不清仍在跑的 PTY 进程与 map 条目；`terminal_runtime.rs:332-337` 二进制垃圾输出逐字节
drain + 整体 clone 重解析；`provider_usage.rs:244-267` `find_meta` 每次重读盘解析 catalog；
`telegram_remote.rs:2918-2926` Permission 回调未回传校验 option_label。（原列「`commands.rs:2034` 起
`install_agent` 是同步 command，分钟级 npm 安装占池槽，应改 async + spawn_blocking」经二次复核为
**误报撤回**：75b952c 当时它就已是 `pub async fn` + `spawn_blocking`（时 commands.rs:403/409），
所引 2034 行实为 `start_acp_session`。）

**Rust 存储 / 解析器 / git**：`util.rs:112`、`gemini.rs:25`、`codex.rs:64` `read_to_string` 严格
UTF-8，一个坏字节整份会话静默跳过（应 lossy 变体）；`storage.rs:947-956` `load_transcript` 整读进内存；
`git_service.rs:827-828` porcelain `&line[..2]` 字节切片理论 panic 面；`grok_dialect.rs:525` 生产路径
`lock().unwrap()`；`context_inventory.rs:681,695` context.json 损坏静默重置并覆盖原文件（应先备份）；
`context_inventory.rs:940` 忽略写失败；`process_util.rs:48-51,67` where.exe/which 无超时；
`memory.rs:137-139` remove+rename 两步间崩溃缺 tmp 兜底恢复。

**脚本 / 构建**：`package.json:9` `verify:text` 对 .mjs 传无效 flag `--experimental-strip-types`；
`start-marionette.bat:46-49` 按 `:5173` 盲杀任意 PID；`src-tauri/capabilities/default.json:4`
`"windows": ["*"]`；`tools/build-portable.bat:298` 硬编码 `C:\tools\upx` 探测路径；
`tools/start-marionette-quick.bat:32` `timeout /t 2` 盲等（应轮询 5173 LISTENING）；
`tools/build-portable.bat:176-181,202` exe 被占用写 `.new.exe` 后仍 `exit /b 0`，发布管线无感知。

---

## 巨型文件拆分方案（子代理提出，按-touch 渐进执行）

- **`App.tsx`（8050）** → `useAcpEventPipeline`（事件管线内聚，对外只暴露 `useSessionEvents`）、
  `useTurnQueue`、`WorkspaceShell`（分屏/拖拽）、`usePromptCards`（三卡 + Esc 层级）、
  SidebarCallbacks 收敛（约 25 个内联回调改 useCallback + `React.memo(ProjectShelf)`，**收益/风险比
  最高，先做这步**）、`useSessionDerivedData`。
- **`Composer.tsx`（3661）** → `useComposerDraft`（含防抖写缓存）、`useAgentCaps`、`ModelMenu`、
  `AgentMenu + useAgentHealth`、`useSuggestions`、`SettingsDialogs`。
- **`acp.rs`（7074）** → `acp_transport`（≈500，纯管道层最易先拆）→ `acp_capabilities`（≈1200 纯函数）
  → `acp_config`（≈800）→ `acp_ui_bridge`（≈870）→ `acp_service`（≈1700）→ 测试（≈2300）移 `tests/`。
- **`model_bridge.rs`（4690）** → `bridge_presets` / `bridge_store`（write_text_atomic 与留档）/
  `grok_surgery`（TOML 外科）/ 按 agent 拆 `zcode_bridge` + `codex_route`。
- **`engine.rs`（3324）** → `engine_install` / `engine_lifecycle` / `engine_api` / 测试外移。
- **`credentials.rs`（1732）** → `secret_store`（DPAPI + 原子写 + 留档）/ `agent_scan`（四扫描器）/ 门面 / 测试外移。
- **`commands.rs`** → `agent_auth` / `agent_install` / `sessions_transcripts` / `media`（独立信任边界）/ `providers`。
- **`app.css`（9564）** → shell-chrome / scrollbars / transcript（可再分 cards+markdown）/
  context-panel / composer / dialogs 六文件，沿既有 `──` 分节切。
- **`parsers/`**：三组重复（SQLite 三兄弟同款 `open_ro`；六个 JSONL 解析器同骨架；两处「预扫前 N 行
  提元数据」各自实现）→ 抽 `sqlite_source` + `jsonl_source(roots, adapter)`，adapter 只写
  `map_event(&Value)`；`deepseek.rs:43` 已示范薄壳复用，推广可把 16 个解析器压成纯映射函数。

## 工程基建空白（第一手确认）

无 CI（`.github` 未跟踪）、无 ESLint/Prettier/Biome、无 `.editorconfig`、无 vitest/jest 单测框架。
质量保障现状 = `cargo test`（后端）+ `scripts/` 下 26 个 `node:assert/strict` 即席脚本 +
`tsc`（藏在 build 里）。直接后果之一即上文 `parsers/mod.rs` 的健壮性错觉：隔离设计只在 debug 构建
成立，却没有一条 CI 通道去跑它。建议最小闭环：GitHub Actions 或本地脚本串
`npm run typecheck`（新增）→ `npm run verify`（聚合 26 脚本）→ `cargo test` → debug 构建
`cargo test` 跑坏输入 fixture。

## 建议的处理顺序

1. **P0 token 清洗**——改动极小（两处错误串），先行。
2. **快赢工程卫生**——lint/test/typecheck 脚本、tsconfig 严格度、`build-now.ps1` 挪 tools/、
   UPX fail-fast、快速脚本 errorlevel、删根目录残留。
3. **Rust 假死面一批**——debug_log 写线程、storage 原子写 + 损坏恢复、git/npm/where 超时、
   桥写互斥、agent_update 超时 kill、poller 出网改入队。
4. **前端热路径一批**——SidebarCallbacks 收敛 + memo（先做）、turnDelivery memoize、rail 增量更新、
   EventRow memo、ProjectShelf 预聚合、Composer 草稿防抖、diff 行数上限。
5. **安全边界收紧**——MarkdownBody 协议白名单、CSP、assetProtocol/read_image 收窄。
6. **巨型文件拆分**——按-touch 渐进，每拆一块跑回归（`npx tsx scripts/verify-*.mts` + `cargo test`）。

## 亮点（保持项，别在重构中丢掉）

- **panic 纪律**：生产路径零 unwrap/expect（unwrap 全在 `#[cfg(test)]`），锁中毒一律转 Err
  （engine.rs 对毒锁还做 `into_inner` 恢复）。
- **stdin 写隔离与断线回收**（acp.rs:3574-3623, 845-886）：pipe 阻塞不挟持调用线程，`on_fail`
  drain 全部 waiter，堵死「UI 永久 Working」。
- **诊断基建**：主线程 watchdog + inflight 命令注册表 + raw stdout tap + panic-hook never_block。
- **密钥卫生**：DPAPI user 作用域加密、mask_key 对外视图、codex 密钥进程内 headers 零落盘。
- **解析器防御一致性**：全 JSONL 守卫跳过式 + BufReader 流式 + 行数上限，对坏行天然免疫；
  `memory.rs:130-141` 的 tmp+rename 是仓库原子写最佳实践（应反哺 storage.rs）。
- **前端细节**：`capabilityCache` 版本号 + 30 天读时清理（全仓缓存最佳实践）；拖拽全程 rAF +
  CSS 变量零中间渲染；时钟全部下沉叶子组件；折叠面板惰性挂载；链接一律 Rust 侧校验 +
  可执行文件二次确认；输入预算意识（200k 截断、图片扩展名白名单）。
- **构建纪律**：批处理逐步查 errorlevel、发布前 `cargo tree` 守门 ring/rustls、panic=abort/
  lto/strip/opt-level=z 一套到位、按版本独立产物目录。

---

## 方法与抽查记录

**分块**：f1 前端会话流/数据层（src/lib 核心）；f2 App.tsx + Composer.tsx；f3 SessionView /
ProjectShelf / ContextPanel；f4 Markdown / 工具卡 / 对话框；f5 样式 / 剩余 lib / 前端构建配置；
r1 Rust ACP 核心与入口；r2 Rust 模型/引擎/凭据桥；r3 Rust 命令面/终端/Telegram/用量；
r4 Rust 存储/解析器/git；t1 scripts / tools / 构建配置。

**运行情况**：10 个 GLM-5.3 只读 run 并行（单个 29-43 次工具调用、context 33-60K）；
r3 首跑被 Zhipu 限流（429 code 1302）打断，串行重跑成功。

**抽查修正**（其余 20+ 处 file:line 论断逐条属实）：
1. f5 称 api.ts「44 处空 catch」——实为「静默兜底返回默认值」的 catch（非字面空块），问题实质成立。
2. r1 称 debug_log 每行 5 次系统调用——info 级已跳过 flush，问题集中在每行 4 次（create_dir_all/
   metadata/open/write）+ 全局锁。
3. t1 称 `diag-zcode.ps1` 仓库 not found——实际文件名是 `zdiag-zcode.ps1`/`zdiag-zcode-out.txt`
   （连同 `zdiag.ps1`、`zdiag-out.txt`、`preview-transcript.local.jsonl`、`.tmp-build.log` 共六件
   本地残留在仓库根），主代理亲验。
4. 次要出入：f2 记 App.tsx 69 个 useState——二次复核 75b952c 时确为 69，f2 本无错（后续提交现为 70）。

**P0 的三方证据链**：telegram_remote.rs 代码两点（2184、3257）→ 本机 Cargo registry 的
ureq-2.12.1 `error.rs:210-240`（Display 拼 URL）→ 日志通路（last_error ≈2226 → log warn
2229-2236 → dev.log）。

**二次复核（2026-10-24，另一会话独立验证约 40 处后回改本文）**：除上述外另修正——规模表
Composer.tsx ≈3100→3661、telegram_remote.rs ≈4300→4639（两者在 75b952c 当时即如此，非漂移）；
App.tsx「≈40 ref」→74、detachedWindow.ts「14 处」→27 处（同，审计时即此数，原为低估）；
撤回 install_agent「同步 command」误报；telegram 退避表述改准（见 P2）；补行号基准注记。
P0 证据链独立复验成立：ureq 2.12.1 Display 拼 URL 的源码在
`D:\Dev\Cargo\registry\src\...\ureq-2.12.1\src\error.rs`（`Error::Status` 打 `response.get_url()`、
`Transport` 打 `transport.url`，两条通路都会把 `bot<TOKEN>` 带进错误文本）。
