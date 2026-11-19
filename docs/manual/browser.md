# 内置浏览器开发手册（browser）

一句话：child WebView2 浏览器标签 + agent 工具面（snapshot/act/batch/waits/dialog）+
元素批注拾取 + ref lease——CDP 纪律是本域生死线。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| surface（child webview/CDP/world） | `src-tauri/src/browser/surface.rs` | `create_surface`/`ensure_resize_hook`/`call_and_wait`/`CdpLease` |
| 模块入口/grant | `src-tauri/src/browser/mod.rs` + `tool_read`/`control_gate` | **grant 检查只有一处** |
| 元素批注拾取 | `browser/pick.rs` + `browser/js/picker.js` | 表达式构建器全 serde_json 禁拼接 |
| ref lease | `browser/ref_lease.rs` | **只判死不判活** |
| typed batch | `browser/batch.rs` | 封闭 op 集 + 每字段白名单 |
| 廉价页面向导 | `browser/js/probe.js`（`browser_page_state/ready/set_value`） | **别往 agent.bundle.js 里加** |
| 工具 schema/dispatch | `tools_mcp.rs`（`tool_schemas()` 单一真源） | browser_help 从 schema 生成 |
| 前端宿主 | `src/components/BrowserPage*` | 标签条在 DOM（host 外）；拾取抽屉在 bar 与 host 之间 |
| aria 引擎 bundle | `browser/js/VENDOR.md` + `agent.bundle.js` | vendored esbuild 产物 |
| fixtures | `tools/browser-fixtures/`（10 页）+ `tools/serve-browser-fixtures.mjs` | 真机手工清单用 |

## 红线（绝不能做——每条都付过学费）

- **一次只允许一个 CDP 会话**（`CdpLease` 进程级）：`call_and_wait` 等待时在泵消息
  循环，第二个 CDP 调用挤进来**重入 WebView2 派发并永久冻住主线程**（2026-11-15
  实锤 watchdog）。新加发 CDP 的命令必须 `CdpLease::take()`；忙时返回可转述的值，
  别排队。
- **快照的 absent 哨兵不能当 JSON 解析**：引擎不在文档时 world 回裸字符串
  `"absent"`——`WorldAnswer::{Absent,Json}` 先分支再 `from_str`（否则 browser_snapshot
  每次必失败）。
- **resize hook 是生存条件**：wry 0.55.1 在任何 child webview drop 时无条件摘宿主
  resize subclass——`ensure_resize_hook` 必须在第一个浏览器标签前装上
  （`create_surface` 自保证，**别绕开它建 child webview**）。升级 vendor/wry 前重测
  「开关浏览器标签后主窗 resize」。
- **world/binding 名字保留 codeg**（`WORLD_NAME`/`__codegSend`/`__codegAgent`）——
  bundle 是上游压缩产物，改名=重放上游构建。
- **持 registry borrow 时绝不 CDP 等待**：先 clone `Rc<Surface>` 出借用再干慢活；
  wry WebView 非 Send，外部一律 `run_on_main_thread` + oneshot。
- **任何 child webview 的创建只准在事件循环内的短路径**；CDP world 懒装到首次读取。
- **`on_cdp_event` 永远零 CDP**（事件可能在别的调用的 pump 里触发）：pick/标题/SPA
  route 只更新内存 + emit；helper `nav-state` 只分类，禁止 `call_and_wait`。
- **node→ref 反向映射只有一条路**：读 `_ariaRef` expando + `elementForRef` 校验；
  **别给 bundle 加函数**。
- **generation 用快照返回的四段串**（`GENERATION.seq.hostGen.navEpoch`）原样传——
  host 的 epoch 是两段，混用全拒。act 不推进 generation（一张 snapshot 覆盖任意多
  连续动作；回退开关 `MARIONETTE_BROWSER_REF_LEASE=0`）。
- **lease 只判死不判活**：host 只对亲眼所见的死亡短路应答；其余交页内
  `refsAreCurrent` 六重校验裁决——**别给 lease 加 is_valid 判定**。
- **batch**：封闭 op 集 + 白名单（未知 op/字段=schema 错误，**没有任何 shell/evaluate
  出口**）；≤32 步；snapshot 只能最后一步；每个 Control 步走 `control_gate`；
  **权限拒绝永远停批**；权限等待冻结批预算。
- **waits**：轮询睡在 MCP worker 线程（`poll_until`），**绝不睡主线程、绝不持
  CdpLease 睡**；必有硬 deadline；超时返回观察结果不是盲目重试信号。
- **dialog**：`Page.javascriptDialogOpening` 分支零 CDP；dialog 打开时所有
  renderer-bound eval 快速失败 `dialog_blocked`（被阻塞 renderer 上的 eval 会挂死在
  进程级 lease 里）；`browser_dialog` 走浏览器进程级 handle，**永不自动应答**。
- **写控件必须真触发页面监听器**（`browser_set_value`：复选框走 `click()`、input 走
  原生 value setter+input/change）——只赋 `checked` 会过值检查却静默失效。
- **grant 一处原则**：读级走 `tool_read`、Control 级走 `control_gate`；「拒绝是返回值
  不是报错」（`{"error":"browser_grant_required"}`）。pick 三/四命令是 UI 路径
  **不加 grant**；always-approve 只跳过 agent 自开标签的 Control 卡。
- **只做内置开发浏览器**：Chrome/Edge 扩展、relay、配对、外部 MCP 浏览器连接都不是
  产品范围；**不加「连接我的浏览器」假入口**（边界见 plan-browser-scope.md）。
- **共同标签必须在原生 host 外**（标签条是 DOM）；dev server 只能扫运行中 ACP
  terminal 的 bounded tail，禁止仅按 localhost 字符串切界面。
- **handoff**：Rust 阻塞等 Continue/Deny/timeout；Deny/timeout 是工具返回值，
  不伪造成功。
- **提示词安全**：页面文字/selector 全部标为不可信数据；URL 剥离凭据/secret
  query/JWT；单批上限 24（前端/页面脚本/Rust ring 三处一致）。
- **快照只覆盖主 frame**（bundle 无 contentDocument）——**别在 host 侧造跨 frame 的
  ref 魔法**（偏差记录在 docs/research/ego-lite/README.md）。

## 坑

- pick_resolve 每次**只拍一张快照**（120K chars）不按 pick 逐拍；裁剪三步
  （captureHide→Screenshot→Restore）一个租约。
- `browser_open_tab` 默认 `background`；fresh tab 只有 `display:"workspace"` 才激活
  surface；稳定 `id`（小写 slug）重复打开返回现有 tab。
- 删除会话/项目关闭其 owned tabs；自动清理只接受普通 Agent tab（busy/waiting_user/
  handoff/当前可见/pinned/claimed 一律跳过）。
- Browser URL 只接受 http(s)，保存前去掉凭据/敏感 query/hash。
- 拾取期间 host 是全屏命中层，底层元素只做 `elementFromPoint`——「指着」绝不能「点了」。

## 验证

```bash
cargo test --features engine-rust --bin marionette browser    # surface/pick/refs/batch
cargo test --features engine-rust --bin marionette tools_mcp
npx tsx scripts/verify-browser-refs.mts      # 源码钉死（bump 只在守卫内/bundle 五重校验——升级 bundle 前必跑）
npx tsx scripts/verify-browser-pick.mts
npx tsx scripts/verify-workspace-surface.mts
npx tsx scripts/verify-dev-server.mts
node tools/serve-browser-fixtures.mjs        # fixture server（真机手工清单用）
npx tsc --noEmit
```

## 已知差距与源流

- 设计稿：`docs/plans/plan-browser-computer-use.md`（§1/§6/§7 偏差记录）、
  `plan-browser-scope.md`；ego-lite 调研：`docs/research/ego-lite/`。
- 差距：same-origin frame 快照补齐/OOPIF/download/popup 被 vendored engine 阻塞。
- 蒸馏自：AGENTS.md「内置浏览器标签/Workspace Surface/元素批注×2/ref lease 与
  browser_help」段 + browser/ 模块头注。
