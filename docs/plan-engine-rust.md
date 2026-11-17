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

## 进行中（子代理）

- [ ] **T1 translate.rs**：chat ↔ responses ↔ anthropic 请求互转 + SSE 事件
  映射 + peel（[1m]/[effort] 剥离）。验收：`cmd /c "check.bat test --bin
  marionette-engine translate"` 绿。
- [ ] **T2 quotas.rs**：真实额度拉取（首类：DeepSeek key 余额），60s 缓存，
  force 跳过；routes 接线。验收：`... test quotas` 绿。

## 待做（每晚自动化按序领取）

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
