# Howto：给引擎加一个订阅账号桥（engine_gateway）

> 场景：某家 agent 厂商提供**订阅制账号**（登录态/订阅钥匙，不是 API key），引擎要像
> magpie 一样把它的对话出站、额度行、拒答方言都接进来。参照物：`grok_account.rs` /
> `codex_account.rs` / `claude_account.rs` / `zcode_account.rs` 四件套。
> 深度索引见 [../engine-gateway.md](../engine-gateway.md)；Go 真相源在
> `third_party/magpie/internal/gateway/<对应>.go`，**逐条对照文件:行号移植，别自创行为**。

## 0. 先问三个问题（决定工作量）

1. **它有没有登录态/凭据文件？** → 决定要写凭据发现（只读）吗——参照
   `zcode_account.rs` 的两来源发现 + `claude_account.rs` 的 `.credentials.json` 读法。
   红线：**绝不写/刷新任何凭据文件；令牌值永不进日志与错误文本**。
2. **它走什么出站协议？** chat 直通（grok）/ 自家协议翻译（codex 的 responses）/
   CLI 子进程桥（claude）？没有 CLI 就没有 claude 形态，别硬套。
3. **额度从哪读？** HTTP 端点（zcode/grok）、CLI 子进程（claude 的 `-p /usage`）、
   还是没有？没有就不出额度行，**不编数**。

## 1. 账号模块 `<vendor>_account.rs`

新建模块，头部必须包含（照抄四件套的形状）：

- **模块头注**：Go 对照清单（哪个文件、移植了哪些函数、**差距清单**——没搬的逐条列，
  接手的人按图索骥）。
- **测试缝**（`#[cfg(test)] Seam`）：注入 config_dir/home/假 CLI/假端点，默认读不到
  任何真实账号。沙箱测试要动 `XDG_CONFIG_HOME` 必须拿 `mod.rs::TEST_SANDBOX_LOCK`。
- **`intercept(gateway, request, proto) -> Option<Reply>`**：路由命中本账号模型时接管。
  挂点在 `routes.rs` 对话臂（现有四臂：chat 142 / responses 159 / anthropic 186 一带，
  每臂在 codex/grok 之后按需追加）。
- **`quota_row`**：额度行来源，接进 `quota_extra.rs::collect`（现有 zcode 一例；
   并行抓取的 scope.spawn 形状照抄）。失败安静出 None——各路互不拖累。

## 2. 厂商 preset（如果注册链要收它）

`vendor_presets.rs`：端点知识表（chat/responses/anthropic 三端点 + regions）。
**端点逐条照抄上游 `internal/provider/presets.go`，改端点先对上游**。表外名字走
自定义来源老路径，不用动。

## 3. 拒答方言（最难也最容易漏的部分）

订阅账号的拒答有自家词表/状态码语义。参照 zcode trial 的形状（2026-11-18 收尾2 批）：

- **纯词表函数**（`trial_refused` 形态）：只看 body 词形，1:1 搬 Go 词表，
  带测试向量。
- **多道门包装**（`trial_refused_for` 形态）：来源门（provider id）/候选门（模型名
  形态）/status 门/点名核对（`eq_ignore_ascii_case`——Go EqualFold 的 ASCII 折叠
  近似，差异如实注记）。
- **rest 分级挂点**：`chat.rs` 候选循环 `Attempt::Retry` 分支——命中只 rest
  `provider::model` 模型级条目（`rest_entry`），判不中回落原行为
  （429/5xx 整 provider、400 不 rest）。**别为一家厂商改掉别家的 rest 语义。**

## 4. 红线清单（写代码前贴在脑门上）

- 不加依赖；子进程只走 `std::process`；锁内绝不做网络 IO/子进程等待。
- 401 永不换账号（各家自己续登录）——codex 实锤的语义，别当通用重试。
- 出站体先过 `bodyprep::try_prepared_request`（routes 臂已统一做，intercept 里
  拿到的已是预处理后的体）。
- Go 有而本批没搬的（缓存/重试/观察端点）→ **差距清单如实列**，别静默假装有。

## 5. 验证

```bash
cd src-tauri
cargo test --features engine-rust --bin marionette <vendor>   # 模块测试（含缝注入的假端点/假 CLI）
cmd /c "..\check.bat check --features engine-rust"            # 编译绿
cd ..
node scripts/verify-engine-contract.mts src-tauri/target/debug/marionette-engine.exe  # 合同面不回归
```

真机：配置一个真实账号 → 一轮对话（流式+工具轮）→ 额度卡出行 → 拔钥匙看拒答词表
命中与 rest 分级行为。真 key 对账项记入 `docs/plans/plan-engine-rust.md` T13 清单。
