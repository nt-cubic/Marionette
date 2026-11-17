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
- [x] 合同文档与套件随分支携带（docs/plan-engine-contract.md +
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

## 剩余（需要人参与的收尾）

- [x] **T13 真机对账——自动化部分**（`2b5c70d7`，2026-11-17 深夜）：
  隔离 profile 起 rust SKU 实例（demo-instance 机制，与用户实例共存）+
  CDP 9223 驱动，**15/15 全过**：引擎进程内起线程（running/rust-0.8.1/
  动态端口）、自定义来源经**真实注册链**落地（providers.json 以 magpie
  兼容形状写盘）、C2 窗口/输出/模态转述、C5 非流式+SSE、C3 形状、
  engine_quota 命令。回归脚本：`scripts/verify-rust-sku-e2e.mjs`（启动
  方法：demo-instance + `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223`，
  **别混 --lang**——组合参数下调试口不监听，T13 实测）。顺手修：rust 态
  place_and_prep 补建引擎目录（注册账本 warn 根因）。
- [ ] **T13 真机对账——真实 key 部分**（自动化做不了，需要用户）：
  1. 真实来源一轮对话（DeepSeek/ollama 任一）：目录窗口对得上厂商规格；
  2. codex 真实会话一轮（/v1/responses 流式+工具轮——T15 的真机首验）；
  3. grok/dsh 跨用桥指向 rust 引擎各一轮（chat 直通 + cordis.patch 表）；
  4. usage 台账四桶与 magpie 基线对比（同来源同模型各一轮）；
  5. 额度卡六家来源对账（有哪家配哪家）。
- [ ] **T14 退役收尾**（T13 绿后）：
  1. 前端 locale「检查 magpie 稳定版更新（约 48 MB）」hint 在 rust SKU
     下不渲染（T12 遗留，改 fe-locale 片段按 SKU 条件）；
  2. 默认 SKU 翻转决策（build-portable 默认 rust、magpie 降为
     `MARIONETTE_ENGINE=magpie` 回退）——**产品决定，留给用户**；
  3. third_party/magpie 与 tools/magpie-seed 退出默认构建路径后留档
     一个发布周期再删；
  4. AGENTS.md「上游移植管线」小节改写（upstream-watch 从「移植门禁」
     转为「行为哨塔」——digest 命中合同区时登记移植任务）。


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
5. 合同面变更必须同步 docs/plan-engine-contract.md。
