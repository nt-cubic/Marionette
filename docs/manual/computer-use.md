# Computer Use 开发手册（computer-use）

一句话：受控的屏幕读取与操作（agent 工具）——helper 子进程 + 驱动 exe，
封闭 Op 集与按键白名单是安全边界。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 协议（8 个封闭 Op） | `src-tauri/src/computer/protocol.rs` | **主进程→helper 的全部出口** |
| helper 子进程 | `computer/helper.rs`（DriverProc::spawn） | exe SHA 每次启动前校验；helper=主 exe 参数分支（main.rs 顶部，单实例锁之前） |
| grant | `computer/mod.rs` 的 `ensure_read`/`ensure_control` | **一处原则**；agent 请求走对话内权限卡 |
| 按键白名单 | `computer/keys.rs` | 表驱动测试 |
| 窗口/共享范围 | `computer/win.rs`（`is_own_image`） | 镜像路径+marionette.exe 名兜底防 dev/正式并装 |
| 驱动 pin | `computer/driver.rs` | 升级 driver=改 pin 常量重审；zip 是 6 文件整包（系统 tar 解压） |
| MCP dispatch | `tools_mcp.rs` | 只调 ensure_read/ensure_control，**别写第二套** |

## 红线（绝不能做）

- **封闭 Op 集**：主进程→helper 只有 protocol.rs 那 8 个 op；helper→driver 的工具名
  硬编码在 `helper.rs::driver_call` 与各 op 里——**任何地方不得把 agent 给的字符串
  透传成 driver 工具名**（driver 原生广告杀进程/录输入回放等几十个工具）。
- **exe SHA 每次启动前校验**：装完校验挡不住之后被换文件。
- **grant 检查只有一处**；agent 请求走对话内权限卡、always-approve 自动放行。
- **自己的窗口/黑名单永不共享**：`is_own_image`（list 与 share 两处都过）+
  `keys.rs::DEFAULT_BLOCKLIST`（系统设置/凭据 UI/密码管理器族）。
- **按键白名单在 keys.rs**：粘贴拒绝 +「agent 自写剪贴板且序列号未变」例外
  （`GetClipboardSequenceNumber`）；桌面和弦（Win*/Alt+Tab/Ctrl+Esc/Ctrl+Alt+*）
  App 级不放；**Win+L/Win+Tab/Ctrl+Alt+Del 任何级不放**。
- **helper 死循环 = stdin 关闭**（Marionette 死 → helper 退 → driver 被 Drop 杀）。
- **锁红线**：`HelperIo`（writer+pending）是独立 `Arc<Mutex>`——等应答的请求方**绝不
  持 STATE 锁**，读线程路由只碰 IO 锁（自锁实锤：「helper did not answer configure
  in time」）。
- 已安装的 `install` 早退也必须 emit "installed"；`computer_list_shareable` 列表失败
  降级为 `helperError`，**永不误报未安装**。

## 未开的面（协议有、UI/检查没接——做之前重读 plan §7）

App/Screen 共享范围（整屏必须连 paint_over 一起做）、指示条 overlay、foreground
交付、verify/launch_app/hold_key/invoke_menu/set_window_frame 工具。

## 验证

```bash
cargo test --bin marionette computer    # keys 白名单表驱动 + op 面
npm run build
```
真机：安装 driver → 列表/共享一轮 → 黑名单窗口拒绝 → 粘贴拒绝（序列号例外放行）。

## 已知差距与源流

- 设计稿：`docs/plans/plan-browser-computer-use.md` §2 + §7 偏差记录；
  行为在 `docs/CURRENT.md`。
- 蒸馏自：AGENTS.md「Computer Use（2026-10-31，批次 3）」段 + computer/ 模块头注。
