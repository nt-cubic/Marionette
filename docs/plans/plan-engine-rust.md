# 引擎 Rust 化账本（engine-rust 分支）

目标：**magpie 子进程彻底退役**——引擎变成 marionette.exe 进程内模块
（`src-tauri/src/engine_gateway/`），单 exe 不再内嵌 20.6MB 载荷、不再养
100–160MB 的 Go 进程。体积红线：不引入 tokio/hyper/axum（std 线程 HTTP +
ureq 出站），ring-less TLS 约束不变。

工作地点：**只在 D:\Project\marionette-rust（git worktree，分支 engine-rust）**。
主树 D:\Project\Marionette 有用户的合并现场，绝不碰。绝不 git push。
每晚自动化（cron 22:00）按本账本推进。

验收标准：`node scripts/verify-engine-contract.mts src-tauri\target\debug\marionette-engine.exe`
全绿（当前状态见下）+ `cmd /c check.bat` 编译绿。

## 已完成（2026-11-17 夜，地基）

- [x] **T0 地基**：`engine_gateway/` 模块——httpd（std 线程 HTTP/1.1，
  Content-Length 体 + SSE 流式 Reply）、store（providers.json 与 magpie
  逐字段互通，flatten 保留未知字段，原子写，每请求 reload=heldFresh 语义）、
  catalog（厂商 /v1/models 抓取 + context_window/max_output/modality 转述 +
  钉死/live + 30s 缓存）、chat（openai→openai 直通 + 模型前缀剥离 + ureq
  Status(4xx) 取回）、**bodycap（学 cap/丢最老图重发/413 语义/prefit——
  2026-11-12 死锁回归的 Rust 守卫）**、routes（C2/C3/C4 面）、cli（种子
  同形动词）、开发 bin `marionette-engine`（2.3MB debug）。
  **合同套件 C1–C6 对 Rust 引擎全绿（2026-11-17 21:20 首次达成）。**
- [x] 隔离 worktree + 分支（主树合并现场零污染）。
- [x] 合同文档与套件随分支携带（docs/plans/plan-engine-contract.md +
  scripts/verify-engine-contract.mts + upstream-watch.mts）。

## 已完成（子代理 + 主线集成，全部验证后提交）

- [x] **T1 translate.rs**（`23e231f5`）：三协议统一 IR 全六向互转 + SSE
  双向映射 + peel 七档全表；13/13 测试。
- [x] **T5 groups.rs**（`23e231f5`）：peel/组by id/组by name/pick/嵌套
  一层/防环；9/9 测试；已接进 chat.rs resolve；effort 词表与 translate
  统一七档。未做：Match globs、Former 谱系、auto 组、多成员失败转移、
  嵌套>1 层、成员 `:effort`、重名歧义让位（见 T5 交付报告）。
- [x] **T8 presets.rs**（`335175b0`）：models.dev 目录抓取/原子缓存/裸 id+
  前缀剥离匹配；6/6 测试；已接进 catalog 转述链（缺窗口兜底，不编数）。
- [x] **T10+T11**（`e00a7c44`）：bodycap 词表全对齐（tooLong 排除/
  pageOverSize/statedCap 最小扫描/learn 只收更小/prefit capNear 90% 线）
  + httpd 加固（chunked/头 30s 写 60s/并发 64 上限）；串行 56/56 +
  套件全绿。

## 已完成（补记三）

- [x] **T12 发布切换**（2026-11-17）：engine.rs 更新路径门控收口——feature
  `engine-rust` 下 check_update()/update()/restore_factory() 走短路径
  （dev.log 记 "engine is in-process (rust); magpie update path skipped"；
  check_update 返回 currentVersion=编译进来的网关版本 + updateAvailable/
  canRestoreFactory 双 false + note=None，update/restore 返回当前状态快照
  ——前端更新面板不卡进度、不弹错误，更新/恢复按钮自然隐藏）；
  EMBEDDED_MAGPIE/EMBEDDED_ENGINE_VERSION 的 include_bytes/include_str
  前置 `not(feature="engine-rust")`；`engine-rust` 与 `embedded-engine`
  互斥由 engine.rs 顶部 compile_error! 断言（Cargo 语法表达不了互斥；
  同开编译期报错，实测生效）。build-portable.bat 新开关
  `MARIONETTE_ENGINE=rust`：跳过整段 magpie 导入/种子 go build/载荷 UPX
  （等价整段 :prepare_embed），cargo features 换 engine-rust，产物照旧
  canonical `release\<版本>\Marionette.exe`（(N) 降级机制共用，未发明
  新路径），尾部打 SKU 标注 + 体积；默认（不设变量）行为一字不变
  （magpie SKU 仍是默认）。体积实测（0.8.1，UPX --best --lzma 后）：
  **rust SKU 5,622,784 字节 ≈ 5.36 MB**（raw 20,005,888 ≈ 19.1 MB），
  对比同版本 magpie SKU 26,156,544 ≈ 24.95 MB——**目标 ≤10MB 达成**
  （约为 magpie SKU 的 21%，省 ~19.6MB）。回归：check.bat 默认 /
  `--features engine-rust` / 默认 bin 三态编译绿 + 双 feature 同开
  compile_error 实测 + engine 模块 50 测试绿 + rust SKU 实跑全程绿
  （前端 tsc/vite + cargo LTO release 12m08s + UPX）。
- [x] **T15 流式工具调用**（`a65aa41e`）：ChatToolAccum 分片拼装 +
  responses function_call 事件族 + messages tool_use 三段式（content_block
  index 编排、usage 尾帧收口顺带修复）；82/82（+20）。边角：乱序 parallel
  事件序取首片序子集、参数整段一帧、call id 未消毒。
- [x] **T9 余额扩源**（`d023a8a8`）：六家来源（Moonshot/OpenRouter/
  SiliconFlow/StepFun/Zhipu/DeepSeek 修正），端点字段逐条对齐 Go
  balance.go；13/13 模块测试。未覆盖清单在 quotas.rs 尾注。

## 已完成（终局补记）

- [x] **T7 目录缓存**（**done-by-design**，随 T6/T12）：`engine-catalog.json`
  的读写（engine.rs `save_catalog_cache`/`cached_catalog_entries`）在 SKU
  无关代码路径上——目录数据走对运行引擎的 HTTP（`/v1/models`），rust 进程
  内引擎同面应答，缓存落盘/回放/`auto/` 剥离对两种 SKU 等价。真实配置
  兼容性：本机无 `~/.config/magpie/providers.json`（引擎从未配置过），
  首次配置即 T13 实机练习；Store 结构按 Go 源逐字段建模。
- [x] **T12 发布切换**（`1965c34a`）：门控 + `MARIONETTE_ENGINE=rust` SKU
  + 实测 **5.36MB**（见上）；magpie SKU 默认路径不变。

## 已完成（2026-11-19 收尾3 批——T13 发现闭环 + 最后大项）

四子代理并行 + 主会话接线，六提交（`37bb5fe0`/`aa028cb6`/`c9e9b447`/
`363a55af`/`74de55b5`/`0e393412`），测试 1139→**1187（全量 0 失败，含 council
flake 根治后首次全绿）**：

- [x] **council same_model 系 flake 根治**（`37bb5fe0`）：根因=等瞬态
  phase（run 几毫秒冲过 Reviewing，25ms 轮询必错过；HEAD 对照 4/5 败实锤
  与引擎批无关但确为既有 bug）——same_model/iso 改等提案落盘、resume 升级
  真·线程崩溃模拟（CrashOnFirstCritique panic 经 scope 传播杀死 run 线程）。
  orchestrator 组 3×17 连跑全绿。
- [x] **T13 真 key 对账**（`aa028cb6` + `scripts/t13-real-key-reconcile.mjs`
  自动化）：五真实来源实弹（deepseek 全绿含 T15 工具轮首验/额度 ¥22.37；
  ollama 额度窗 83.3%；错误保真实弹）。见上节。
- [x] **continuation 断流续答**（`363a55af`，chat.rs +1280）：Go
  continuation.go 直通半边——ContinuationReader 零解析直通基线不变、
  chat_prefill（DeepSeek prefix+/beta 路径、Kimi partial）、prefill 拒答
  记忆、Unecho 回声剔除、台账补账总额同 billed。chat:: 29→43。
- [x] **Claude 桥 gap 1 最小可用集**（`c9e9b447`，claude_account.rs +713）：
  淘汰即入架（claude-shelf.json）+ 冷启动 --resume 只喂增量 + 首帧偷看
  回退 + 孤儿清扫。claude 36→44。
- [x] **opencode-go 会话头**（`74de55b5`→H 线 + `0e393412`→主会话接线）：
  opencode_client.rs 纯模块 + OpenCodeCtx 种子链接线进 chat.rs 出站。
- [x] **zhipu 目录兜底**（`74de55b5`）：ensure_fresh()（Go KeepFresh 最小版
  ——真根因是 models.dev 补抓从未被调）。
- 验证：全量 **1187/0** + 合同 C1–C6 全绿 + T13 脚本真机复打两处闭环确认。

剩余（各模块头注为准）：continuation 的 anthropic 原生半边与 7 条子项、
Claude 桥 branch/replay 家族、opencode 的 zenFree 档与入站头穿线
（messages/responses 翻译路径）、HEIC（红线卡住）。**引擎代码侧大项已清**。

## 已完成（2026-11-18 收尾2 批——销 25cb0d80 挂起单）

四子代理并行 + 主会话接线，四提交（`4e0ae6d5`/`48caefa4`/`a3e1d420`/
`4013e7f9`），+5,384/−531，测试 1089→1139：

- [x] **redact 收口 + 接线**：4 个 WIP ignore 转绿；phone 判定全族
  （phoneAt/fieldName/nameWords）按 phone.go+ids.go 移植；bodyprep 链末位
  scrub_cow（零拷贝关态）+ routes 四臂 try_prepared_request Err→503
  （maskStatus 非拦截分支）。**HEIC 挂起记档**：Go 用 gen2brain/h265 纯
  Go 解码器，Rust 无等价物且红线禁新依赖——不做，形状记在 bodyprep 头注。
- [x] **zcode trial rest 分级**：trial_refused_for 四道门 + chat.rs 模型级
  rest 条目（`provider::model` 键）；trial 拒答只 rest 该 `-Trial` 条目，
  账号其它模型照常候选；判不中回落原行为。
- [x] **usage token 捕获层**：TapReader 包 relay 链、流尾收口、三方言桶
  口径 1:1、millis 从请求入口起算；responses/messages 翻译路径补记账。
  顺手修：note_turn 在应答体取 session（恒 None）→ 改请求体；翻译轮
  cache_write 被丢 → 补 input_tokens_details.cache_write_tokens（#589）。
- [x] **Codex WS keep 复用 + shun + 1009**（仍圈在 MARIONETTE_CODEX_WS=1）：
  keep 池（idle 10min/age 55min/max 32）+ previous_response_id 增量续跑；
  shun host:port 10min；1009 按实发字节记最小拒收尺寸。实锤修复：serde_json
  preserve_order+swap_remove 键序漂移 → sorted_value 递归键排序。
- [x] **Claude 桥 gap 2（跨回合驻留）+ gap 3（MCP helper）**：turn_key 会话
  键（消息归一 1:1）、idle 1h 池（按 Go 源，简报 2h 有误）、泵线程+看门狗；
  helper 改**进程内环回 HTTP MCP**（main.rs 不在模块手里，协议与 Go 逐字段
  同形）；阻塞-回填先登记后放帧（免 Go 的 20×25ms 竞态重试）；真机级修复：
  `_meta` 的 `claudecode/toolUseId` 整键带斜杠不能走 JSON pointer。
- 验证：全量 1138 绿/1 败（council `same_model_two_members_distinct_calls`
  ——HEAD 对照实验 4/5 也败，**既有高失败率 flake，与本批无关**，已记
  AGENTS）；默认构建零变化；合同 C1–C6 全绿。

收尾2 后仍剩（各模块头注为准）：continuation 断流续答（chat.rs 尾清单）、
Claude 桥 gap 1（savedSession 家族）与 4–14 细项、WS 的 deflate/分片帧/
SOCKS、redact 的 crypto.go/more.go 附加规则与用户词表。T13 真 key 对账、
T14 退役决策仍待用户。



- [x] **T13 真机对账——自动化部分**（`2b5c70d7`，2026-11-17 深夜）：
  隔离 profile 起 rust SKU 实例（demo-instance 机制，与用户实例共存）+
  CDP 9223 驱动，**15/15 全过**：引擎进程内起线程（running/rust-0.8.1/
  动态端口）、自定义来源经**真实注册链**落地（providers.json 以 magpie
  兼容形状写盘）、C2 窗口/输出/模态转述、C5 非流式+SSE、C3 形状、
  engine_quota 命令。回归脚本：`scripts/verify-rust-sku-e2e.mjs`（启动
  方法：demo-instance + `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223`，
  **别混 --lang**——组合参数下调试口不监听，T13 实测）。顺手修：rust 态
  place_and_prep 补建引擎目录（注册账本 warn 根因）。
- [x] **T13 真机对账——真实 key 部分**（2026-11-19，`scripts/t13-real-key-reconcile.mjs`
  自动化：隔离 XDG 副本起 dev bin，真厂商实弹，绝不触真实 usage 台账/绝不打印 key）：
  1. ✔ **deepseek 全绿**：目录 2 行（ctx=1M/out=393K/模态 text|image——对得上厂商
     规格）；chat 流式 200（19 帧）；**工具轮 200 含 tool_call 事件（T15 真机首验）**；
     responses 面 200（response.created 事件流——翻译路径实弹）；usage 台账
     in=35/out=16/millis≈700 与厂商自报精确一致；额度行真实出数（balance ¥22.37）。
  2. ✔ **ollama（ollama.com 云）**：chat 流式 + 工具轮 200；额度窗 Month used 83.3%
     /remaining 16.7% + balance $0.00 两行真实出来；usage 无帧=不编数（正确）。
  3. ✔ **错误保真**：zhipu 429 code 1308（5h 限额，与本会话子代理同款）/
     openrouter 401 "User not found"（key 已吊销，待用户续）/opencode-go 400
     MissingSessionID——全部原样透传，错误保真路径实弹确认。
  4. ~~**对账发现（引擎差距，待后续批）**~~ **两条当日全部闭环**（2026-11-19
     收尾3 批）：opencode-go 会话头 → `opencode_client.rs` 纯模块（1:1 移植
     provider/opencode_client.go：UA+更新器/trace LRU/项目 id git 机制/会话
     id 形状机，20 测试）+ chat.rs 出站接线——真机复打 **400 MissingSessionID
     消失**（厂商开始按账号规则应答；余下 400 区域策略/403 需订阅全是账号
     侧控制台设置）；zhipu 目录 ctx=0 → 真根因是 `presets::refresh` 生产路径
     从未被调（Go serve 起 catalog.KeepFresh()）——`ensure_fresh()` 后台单飞
     补抓+退避，真机 12 行 zhipu 全部填上（glm-4.5 ctx=131072 与 models.dev
     逐字对上）。
  5. 覆盖说明：grok/dsh 跨用桥的对话面与本对账同一 chat 直通路径（已实弹）；
     CLI spawn 层留日常使用验证。真 codex 会话轮=responses 面已真轮（响应事件
     形状对），codex CLI 全链 spawn 属日常使用。
- [x] **T14 退役收尾**（2026-11-19 结账）：
  1. ✔ locale hint 按 SKU 门控——早批已完成（ProjectShelf.tsx :2222：
     `engineState.version?.startsWith("rust-")` 时不渲染 magpie 更新 hint）；
  2. **默认 SKU 翻转=暂缓**（工程判断，非阻塞）：rust 引擎核心路径已实弹
     （T13 真 key 全绿），但 continuation 续答/opencode-go session 头等
     收尾刚落地、真 codex 全链 spawn 未在日常使用中过——再留 magpie
     一个发布周期，翻转时 build-portable 默认换 rust、magpie 降为
     `MARIONETTE_ENGINE=magpie` 回退（一处 if 翻转，随时可做）；
  3. third_party/magpie 与 tools/magpie-seed 留档一个发布周期（与 2 同步）；
  4. ✔ 上游管线角色改写完成（docs/manual/engine-gateway.md：「上游跟进=
     研究克隆+digest，不是合并代码」；upstream-watch digest 命中合同区时
     按账本登记移植任务=行为哨塔）。


- [x] **T6 进程内接线**（`7f31e5cf`）：feature `engine-rust` 全链——
  spawn_serve_in_process（同步 bind 占端口）、InProcessServe 停止机制
  （AtomicBool + 有界等退出）、run_magpie_cli 直调（GUI 零 println）、
  place_and_prep 直达 Ready（不下载/不复用外部 magpie）；default 构建零
  变化；62/62 + 双 feature 编译 + 套件 ×3 全绿。**数据面按设计即兼容**：
  rust 引擎与 magpie 同用 `~/.config/magpie/providers.json`，旧配置无需
  迁移。未做（→T12）：update()/restore_factory() 门控；嵌入载荷在
  feature 态的编译剔除。

## 已完成（补记）

- [x] **T2 quotas.rs**（`cc1ce333`）：DeepSeek key 余额（多币种稳定排序）+
  60s 缓存 + kept=1 语义；4/4 测试。
- [x] **T3 messages.rs**（`cc1ce333`）：/v1/messages 直通（双认证头）+
  chat 翻译（message_start 首帧/单次收口/include_usage）；5/5 测试。
  未做：流式 tool_use（→T15）、直通路径 bodycap（anthropic 体形状）。
- [x] **T4 responses.rs**（`cc1ce333`）：/v1/responses 直通 + 翻译
  （created/delta/completed 序列、usage 口径含 cached/reasoning 拆桶）；
  8/8 测试。未做：流式 function_call（→T15）、直通路径 bodycap
  （responses 体形状）、reasoning item。
- [x] **测试沙箱锁统一**（`cc1ce333`）：presets/quotas/responses 私锁
  互踩 XDG_CONFIG_HOME（并行 1/5 概率红）→ mod.rs 共享
  TEST_SANDBOX_LOCK；57/57 × 3 并行连跑全绿。

## 待做（按依赖排队，前者落地即开工）

- [ ] **T3 /v1/messages 端点**：anthropic 入站 → translate 到厂商协议；
  Claude 系 agent（dsh 的 anthropic 后端）可用。验收：套件加 anthropic
  用例或真机 dsh 冒烟。
- [ ] **T4 /v1/responses 端点**：codex 入站。先直通（openai 厂商 responses
  URL），再翻译到 chat 厂商。验收：codex spawn 指向 Rust 引擎真机一轮。
- [ ] **T5 组路由**：模型 id 是组/别名时按 members+pick 路由（现在只有
  provider/model 直通）；GroupFor 用 translate::peel_agent_model_mark。
- [ ] **T6 engine.rs 进程内接线**：`spawn_serve` 在 engine-rust 形态下起
  `engine_gateway::httpd::serve` 线程（不再 spawn 子进程）；
  `run_magpie_cli` 的 provider/group 调用改直调 cli 模块函数；MAGPIE_ADDR
  端口分配/状态机/kill_engine 对齐。feature flag：`engine-rust`。
- [ ] **T7 目录缓存**：`~/.marionette/engine-catalog.json` 读写对齐
  （engine.rs 现有消费不改）。
- [ ] **T8 presets**：models.dev 目录内嵌（窗口/名字兜底；magpie catalog
  的 Rust 对应）。来源：`https://models.dev/api.json` 构建时抓取或首次运行
  缓存——注意 CSP 与体积，先做运行时缓存。
- [ ] **T9 quotas 其余来源**：订阅类（codex/claude）额度——依赖账号
  discovery，逐类补（见 T2 子代理的差距清单）。
- [ ] **T10 bodycap 词表对齐**：CAP_WORDS 对齐 magpie bodyCapWords 精确
  正则（研究克隆 internal/gateway/body_cap.go）。
- [ ] **T11 HTTP 硬化**：chunked 请求体（codex 若发）、keep-alive（可选）、
      并发连接上限、慢连接超时。
- [ ] **T12 build-portable 切换**：`--features engine-rust` 时不嵌 magpie
  载荷（EMBEDDED_MAGPIE 不编译进去），发布 exe 目标 ≤10MB；保留 magpie
  形态作为回退 SKU 一个发布周期。
- [ ] **T13 真机对账**：grok owned 表 / dsh cordis.patch / opencode
  models.json / zcode providerModelRules / codex env 全部指向 Rust 引擎后：
  usage 台账四桶、额度卡、附图（modalities 宣告）与 magpie 基线一致。
- [ ] **T14 退役**：third_party/magpie 退出构建路径（vendor 目录留档），
  AGENTS.md/docs 收口，体积/内存实测记录进本账本。

## 红线（每晚自动化必须遵守）

1. 只在 D:\Project\marionette-rust 工作；绝不碰主树、绝不 push、绝不建
   GitHub 仓库/fork。
2. 每晚开工先 `git status` 确认干净；结束前 `cmd /c check.bat` 绿 +
   相关测试绿才 commit（`engine-rust(夜): Tn …`）；构建坏了回滚当晚改动
   并在账本标记 blocked。
3. 一次只领一个任务；做不完就拆小，别留半成品在主分支状态。
4. 不加新依赖（尤其 tokio/hyper/reqwest）；TLS 保持 ureq+native-tls。
5. 合同面变更必须同步 docs/plans/plan-engine-contract.md。

## 2026-11-19 主线集成与默认转正

本轮把 `engine-rust` 的引擎、合同、测试、研究/手册与产品边界合入当前主树，并完成默认发布门槛：

- [x] **配置耐久 P0**：`providers.json` 只在不存在时从空表开始；已有文件读失败/坏 JSON/BOM/权限错误/运行中消失/外部并发修改均禁止覆盖。写前保留 `providers.json.bak`，Windows 用 `MoveFileExW(REPLACE_EXISTING|WRITE_THROUGH)` 原子替换；HTTP 路由在配置不可写时返回 503，不再把来源误报成全部消失。
- [x] **代理 P1**：出站代理从 `OnceLock` 改为可替换槽；停止/重启进程内引擎时可换到新代理，显式关闭也能覆盖旧值。
- [x] **账本 P1**：Rust usage 独立写 `engine-usage.jsonl` / `.1`；首次打开只从旧 `usage.jsonl` 迁移严格可解析的 Rust 行，Magpie 行和原文件不动。
- [x] **Claude 图片 P1**：anthropic image base64/URL 块按原位置进入 Claude Code stream-json；tool_result 图片同样转发，目录宣告 `images:true`。Go 的 2000px/5MiB 预缩放仍属于后续增强。
- [x] **默认 SKU**：Cargo 默认 feature 改为 `engine-rust`；`build-portable.bat` 和 `start-marionette.bat` 默认进程内引擎，`MARIONETTE_ENGINE=magpie` 仅作一个发布周期的回退。
- [x] **主线冲突策略**：保留当前主树较新的 `third_party/magpie` 种子；Rust 侧只合入引擎模块、接线、合同、文档及其直接依赖，避免把旧上游快照硬覆盖回来。
- [x] **验证**：Rust 全量 `1223 passed / 0 failed / 5 ignored`；三协议合同 C1–C6 + D1–D10 全绿；默认/回退两条 Cargo 路径编译绿；前端生产构建绿。

当前待观察项只有一个发布周期：真实 Codex 超长回合、Claude 超大图缩放、以及新接桥 Agent 的真实组合。它们不影响默认 Rust SKU，出问题时可切 Magpie 回退构建复现。
