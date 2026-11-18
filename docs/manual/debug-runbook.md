# 排障手册（debug-runbook）

一句话：用户报「卡退 / 没反应 / 打不了字」时的**取证路径与已知根因**。
本手册无自动测试——纪律是「先取证再动手」。

## 文件地图

| 想看什么 | 去哪 |
|---|---|
| 开发者日志 | `%USERPROFILE%\.marionette\logs\dev.log` |
| 日志设施 | `src-tauri/src/debug_log.rs`（cfg(test) 拒绝真实路径，防测试窜流） |
| 数据目录写不进的兜底 | `app_paths::ensure_global_dir_writable` + `data_dir_gate.rs` |
| 单实例交接 | `shell_integration.rs` |
| 崩溃兜底/看门狗/最小化挂起 | `src-tauri/src/main.rs`（is_tao_paint_assert / shutdown_and_exit / MINIMIZE_SUSPENDED / watchdog） |
| 完整性标签修复脚本 | `scripts/fix-workspace-integrity.ps1` |
| PTY 边界修复 | `terminal_runtime.rs`（char boundary 再 drain） |
| vendor 补丁说明 | `src-tauri/vendor/tao/MARIONETTE-PATCH.md`、`vendor/wry/MARIONETTE-PATCH.md` |

## 排障路径（按症状）

### 症状：卡退 / 闪退
1. **第一件事**：dev.log 搜 `[panic]`——release 是 `panic="abort"`，**任何线程
   panic 都杀全进程**，panic 行 = 根因（位置+消息），不用再猜。
2. 附近有 `[watchdog] MAIN THREAD STALLED` = 主线程冻结（假死），与 panic 无关，
   查主线程耗时操作（持锁跑子进程/秒级 IO）。
3. Windows 事件日志（Application Error）退出码：`0xc0000409` ≈ panic-abort
   （fail-fast）；`0xc0000005` = 访问违规。
4. 转储：`C:\ProgramData\Microsoft\Windows\WER\ReportArchive\AppCrash_Marionette.exe_*`
   （Report.wer 有时带堆栈模块偏移）。
5. 判「JS 活没活」：dev.log 每 60s 一条 `ui-heartbeat`（`document.hidden` 时暂停）。

### 症状：exe 双击没反应（无窗口/无日志/退出码 0）
或：`npm run build` 报 esbuild `Access is denied` + `%TEMP%` 残留 `esbuild-*`。
**同一根因：Low 完整性标签。**（沙箱 workspace-write 给工作区加的可继承标签，
沙箱结束后仍留在 .exe 上。）
```powershell
icacls D:\Apps\Marionette | Select-String 'Mandatory Label'   # 看到 Low 即确诊
scripts\fix-workspace-integrity.ps1                            # 等价 icacls /setintegritylevel (OI)(CI)Medium
```
会复发——工作区再被沙箱跑过就再来一遍。产品兜底已加（写探测文件失败弹原生
对话框 exit(1)，不再静默 exit(0)）。

### 症状：还原后键盘失灵（打不了字，其它一切正常）
根因：WebView2 焦点错位（最小化时恰好持焦点的场合，`set_visible(true)` 不做
MoveFocus）。修法在 main.rs：还原分支与 `Focused(true)` 兜底在 `show()` 后追加
`set_focus()`（wry 补丁 #4：可见+Resume+MoveFocus 三合一）。**别删这两处。**

## 坑（踩过的，别再踩）

- **PTY 输出截断 mid-codepoint**：从 PTY 读到的字节可能停在 UTF-8 多字节中间，
  裸 `drain(..n)`/`truncate` 触发 `is_char_boundary` 断言 → abort。修法=回退到
  字符边界；**新代码先保证 char boundary 再切**。
- **tao 0.35 paint assert**（`flush_paint_messages` 重入）：关窗路径 WebView2 双
  pump paint；点击/IME 路径 CJK 输入法嵌套泵（日语系统+中文输入法最易中）。
  关窗路径的 `is_tao_paint_assert` + `shutdown_and_exit` **不要再改**；点击路径
  靠 `vendor/tao` 补丁。Tauri 升到 tao ≥0.36 后删 `[patch.crates-io]` 的 tao 行；
  wry 那条补丁（文字拖放/TrySuspend）升级前同样对补丁文件核对。
- **测试日志窜流**：debug_log 的 `log_path()` 在 cfg(test) 下拒绝真实
  `~/.marionette/logs/dev.log`。回归口径：跑完 cargo test 后
  `grep -c "marionette_test\|9\.9\.9" dev.log` 零增长。

## 已知差距与源流

- 蒸馏自：AGENTS.md「排查卡退」「排查 exe」「还原后键盘失灵」段 +
  内存压力韧性批（watchdog/ui-heartbeat）。
- 新增崩溃案例的纪律：修完在对应模块头注记根因，并回填本手册「坑」节。
