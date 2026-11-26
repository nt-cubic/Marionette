# 内存与进程手册（memory-process）

一句话：进程树卫生（Job Object/孤儿收割）、WebView2 挂起成对纪律、工作集压制、
引擎懒启动——多开不涨内存、空闲压到个位数的全部机制。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| ACP 会话进程树 Job Object | `src-tauri/src/process_util.rs::KillJobGuard`（:891，RAII） | kill-on-close；孙进程（node 的 codex.exe/MCP）继承 |
| 引擎共享 Job + GOMEMLIMIT | `engine.rs::spawn_serve`（magpie SKU） | 崩溃/强杀不留孤儿 magpie；rust SKU 无此进程 |
| 孤儿收割 | `engine.rs::reap_orphan_engines` | toolhelp 快照+父进程已死；**fail-open**（PID 复用宁可误判活） |
| 最小化 → webview TrySuspend | `main.rs`（`MINIMIZE_SUSPENDED` :212） | check-then-clear 红线见下 |
| 空闲会话工作集压制 | 前端 60s 巡逻 tick + `trim_session_working_set` 命令 | `sessionSuspend` 的 TRIM_QUIET_MS=2min；同会话 15min 不重复 |
| 引擎懒启动 | `engine.rs::startup_lazy` | `engine_binding_count()>0` 才 serve |
| 目录缓存/快照保留 | `engine-catalog.json` + `quotaCache.ts` + localStorage `marionette-quota` | 引擎睡眠时菜单/额度卡不消失 |
| 浏览器懒建 | `surface::create_surface`（prewarm 已删） | 首标签冻死的回退=setup 恢复 prewarm |
| 冷淘汰 | `useSessionRails` 的 `isPersistClean` + `pruneColdSessionMemory` | 见 [storage-transcripts.md](storage-transcripts.md) |

## 红线（绝不能做）

- **TrySuspend 必须成对**：tauri 的 `window.hide()/show()` 只走 tao 层不碰 wry 的
  set_visible——**每条隐藏/唤醒路径都要成对补 webview 级调用**（前端
  `getCurrentWebview().hide()/show()`；Rust `win.as_ref().show()`）。**漏 show
  半边 = 唤醒后白屏**。新增加 show 路径时先想到这条。
- **最小化挂起 check-then-clear，绝不先 swap 再查最小化**（清了账本却没 resume = 白屏）。
- **工作集压制只打 waiting 会话的树**：禁止对 running 会话/主进程/WebView2 进程
  调 `EmptyWorkingSet`（活跃流会疯狂换页）。busy 豁免集合（子代理/ACP 终端/
  定时发送/待决权限卡）不压。
- **KillJobGuard 只许 RAII/terminate()，禁止手动 CloseHandle**。
- **不做会话数量上限、不强制挂起**（用户主权，2026-11-06 定案——别再提）。
- **auto/ 全面隐藏的唯一过滤点是 `catalog_direct`/`catalog_direct_entries`**——
  别在消费点各自再滤一遍；读侧也剥 auto。
- **僵尸绑定清扫的空世界守卫**：会话清单为空且绑定非空时**不清**（清单读失败与
  「真没会话」分不清，fail-open 宁可多养引擎不误删）。
- **engine 缝没挂 dir 时 `catalog_cache_path` 必须返回 None**（绝不读写真实
  `~/.marionette/`）。

## 坑

- prewarm 曾泄进注册表出幽灵「浏览器 1」标签（2026-11-01 实锤）——懒建后每个标签
  （含首个）走 `create_surface` 直建；**任何 child webview 的创建只准在事件循环内的
  短路径**。
- 引擎懒启动的**显式入口保留清单**：网关按钮 / `bridge_bind`（唯一合法隐式 enable，
  含首装下载）/ dsh·pi·zcode·opencode 的 spawn wake / council——别把它们也门掉；
  纯浏览菜单不得拉引擎（`bridge_list_models` 的 5s 唤醒同过 binding 门）。
- `document.hidden` 时暂停 3s 终端轮询 / 1s live tick / 1s stats / 12s git 扫描，
  可见即续；**安全类 tick（定时发送、挂起巡检）不受影响**。
- 撕窗 destroy 不做（paint-assert 红线；隐藏已 TrySuspend，够了）；
  `additionalBrowserArguments` 不调（会整体替换 wry 默认参数并分裂 data directory）。

## 验证

```bash
cmd /c "check.bat test --features engine-rust --bin marionette"   # process_util/engine 相关
cargo test --bin marionette catalog_direct_hides_auto
cargo test --bin marionette drop_auto_bindings_clears_only_auto_rows
```
真机手册（docs/CURRENT.md 批次清单）：冷启动无 browser-profiles 进程组；空闲会话
2–3 分钟后工作集回落；挂起杀整树；强杀无 node/codex.exe 残留；最小化/还原白屏检查；
冷会话转录 mtime 不再周期变。

## 已知差距与源流

- 蒸馏自：AGENTS.md「资源优化批」「内存压制批」全文 + main.rs/process_util.rs 实读。
- 硬底线（诚实）：WebView2 运行时本身 ~250MB；Web UI 不弃总量就到不了 100MB——
  换原生 UI 是独立重写项目，别顺手做。
- **地板复测（2026-11-26，v0.8.6 隔离实例，无会话、引擎未启动）**：整棵树
  7 个进程 = 工作集 191MB / 专用（提交）280MB，与上面的口径一致。拆开看：
  主进程 54 / 32MB；WebView2 六进程 138 / 248MB，其中 **gpu-process 一家就占 126MB
  提交但只有 ~9MB 驻留**，浏览器进程 43MB、渲染进程 54MB、工具进程 ~22MB。
  「WebView2 吃 600MB」不是空闲地板，是有内容之后的状态——同一台机器上实测
  Teams 的 WebView2 树工作集 1.1GB（渲染进程 987MB）；内置浏览器标签开了重站点、
  长对话里解码过的图片、代码高亮都会把渲染进程顶上去。对外报数时说清是工作集
  还是专用提交，别混。
- **最小化省内存是有条件的**：同一次复测里最小化后专用只从 281 降到 273MB，
  工作集没降（日志确认 `webview suspended (TrySuspend)` 确实发生）。上面写的
  「再降 150–250MB」只在渲染进程本来就装了很多页时才成立；空实例的渲染进程
  才 47MB，没东西可省。
- **演示实例只隔离数据，不隔离 agent 自动更新**：`tools\demo-instance.bat` 起的
  实例照常执行启动时的 agent 自动更新，改的是**全局 npm 包**（2026-11-26 实锤：
  隔离实例把 `@openma/deepseek-harness-acp` 与 `zcode-acp-server` 升了级）。用它量
  内存或拍图前要知道这一点。
