# Marionette 体积优化方案

> 日期：2026-07-28
> 状态：**方案待评审**
> 目标：将最终 release 从 ~8-9 MB 压缩至 ~4-5 MB，同时清理 PTY 遗留代码

---

## 现状

| 层 | 估算大小 | 说明 |
|---|---------|------|
| 前端 dist（Vite build） | ~896 KB | JS 822KB + CSS 74KB + HTML 0.4KB |
| Rust 二进制（release, stripped） | ~5-8 MB | Tauri 壳 + WebView2 绑定 + 所有库依赖 |
| **合计预估** | **~6-9 MB** | 当前无 release build，基于依赖链推算 |

产品宣称「不到 10MB」，目前大概率在边界上。通过四项措施可以降到 **~4-5 MB**，留出充裕余量。

---

## 措施一：Release Profile 优化

**预估节省：2-3 MB**

当前 `Cargo.toml` 没有配置 `[profile.release]`，全部使用 Rust 默认值（`opt-level=3`，无 LTO，无 stripping）。

在 `Cargo.toml` 末尾追加：

```toml
[profile.release]
strip = true               # 去掉符号表（体积大头）
lto = true                 # 跨 crate 链接时优化
codegen-units = 1          # 单编译单元，给 LTO 最大机会
opt-level = "z"            # 按体积优化（替代默认的速度优化）
```

**影响**：无。纯编译配置变更，不影响行为。`opt-level="z"` 可能略微降低运行时速度（但 Marionette 是 I/O 密集型壳，不是计算密集型）。

---

## 措施二：Tauri Features 裁剪

**预估节省：0.3-0.5 MB**

当前：
```toml
tauri = { version = "2", features = [] }
```
`features = []` 不代表关闭所有——因为没设 `default-features = false`，**9 个默认 feature 全部被开启**。

改为：
```toml
tauri = { version = "2", default-features = false, features = ["devtools"] }
```

砍掉的默认 feature：

| Feature | 为什么能砍 |
|---------|-----------|
| `tray-icon` | 代码中没有 TrayIcon 使用，没有托盘图标 |
| `compression` | 前端 dist < 1MB，压缩收益极小；需要时单独开 Brotli |
| `dbus` | Linux D-Bus，无 tray 无 notification |
| `x11` | Linux 窗口，wry 会自动处理 |
| `common-controls-v6` | Windows Common Controls，无菜单无 tray |
| `dynamic-acl` | 权限全是静态 JSON，不动态修改 |

保留 `devtools` 方便开发时打开 Web Inspector。

**影响**：无。所有被砍的 feature 都经过验证——代码中零引用。

---

## 措施三：移除 PTY / Raw Terminal（最大块）

**预估节省：1.5-2.5 MB**（portable-pty + xterm.js + 桥接代码）

### 前提

代码审查确认：**所有内置 Agent 已默认走 `transport: "acp"`**（见 `models.rs`）：

```rust
// 所有默认 Agent
Self::new("opencode",   ..., "acp", ...);
Self::new("codex",      ..., "acp", ...);
Self::new("claude-code", ..., "acp", ...);
Self::new("grok-build", ..., "acp", ...);
```

PTY transport 是遗留 fallback，用于用户手动配置的自定义非 ACP Agent。当前产品已全部走 ACP 协议。

### 删除清单

#### 整文件删除（3 个）

| 文件 | 行数 | 说明 |
|------|------|------|
| `src-tauri/src/pty.rs` | ~308 | `PtyService` 核心实现，PTY 进程管理 |
| `src/lib/ptyCleanBridge.ts` | ~264 | PTY 输出 → Clean 事件桥接 |
| `src/lib/ptyProfiles.ts` | ~144 | PTY 控制配置表 |

#### 部分修改（16 个文件）

按层级分：

**Rust 后端：**
- `src-tauri/Cargo.toml` — 移除 `portable-pty` 依赖
- `src-tauri/src/main.rs` — 移除 `mod pty`、`PtyService`、`AppState.pty`、shutdown 中的 `pty.stop_all()`、5 个 Tauri 命令注册
- `src-tauri/src/commands.rs` — 移除 5 个 PTY 命令函数（`read_terminal_snapshot`、`start_terminal`、`write_terminal`、`resize_terminal`、`stop_terminal`）

**前端类型 & API：**
- `src/lib/types.ts` — 移除 `"pty"` transport、`"raw-terminal"` viewMode、`ptyId`、`rawLogPath`、`raw_chunk` 事件类型
- `src/lib/api.ts` — 移除 5 个 PTY API 函数
- `src/lib/mockData.ts` — 移除 PTY mock 数据

**React 组件：**
- `src/app/App.tsx` — 移除 `ptyBridgesRef`、`session-output` 监听、PTY 发送分支、`schedulePtyFlush`、PTY 桥初始化
- `src/components/SessionView.tsx` — 移除 `RawTerminal` 组件、xterm 导入、`raw-terminal` 视图模式分支
- `src/components/Composer.tsx` — 移除 `onPtyCommand`、`isPty` 分支、ptyProfiles 导入

**样式 & 构建：**
- `src/main.tsx` — 移除 `import "@xterm/xterm/css/xterm.css"`
- `src/styles/app.css` — 移除 `.terminal-*` 和 `.raw_chunk` 样式
- `package.json` — 移除 `@xterm/xterm` 和 `@xterm/addon-fit` 依赖

**影响范围：** 约 **1600 行删除**，3 个文件删除，16 个文件修改。

### 风险

1. **用户自定义 PTY Agent 将无法使用** — 如果用户手动配置了一个 `transport: "pty"` 的 Agent，移除后该 Agent 会失效。可以接受：所有主流 Agent 都实现了 ACP 协议。
2. **`raw_terminal` 视图模式消失** — Clean View 成为唯一视图模式。产品定位本就是「Clean View 是主界面」，Raw 只是排障旁路，移除后一致性更好。

---

## 措施四（可选）：前端 Brotli 压缩

**预估节省：~200KB（安装包层面）**

在 `tauri.conf.json` 中启用压缩，将嵌入的 900KB 前端 dist 进一步压缩。

当前 `bundle.active` 为 `false`（不打包），无需配置。如果未来启用打包，可打开压缩选项。

**建议：后续需要打包时再开启，现在做收益不明显。**

---

## 总览

| # | 措施 | 节省 | 难度 | 风险 |
|---|------|------|------|------|
| 1 | Release profile | **2-3 MB** | 低（3 行配置） | 无 |
| 2 | Tauri features | **0.3-0.5 MB** | 低（改一行） | 无 |
| 3 | 移除 PTY / Raw Terminal | **1.5-2.5 MB** | 中（~1600 行） | 低（已全是 ACP） |
| 4 | Brotli 压缩 | ~200KB | 低 | 无（可选） |
| **总计** | | **~4-6 MB → 最终 4-5 MB** | | |

三个主措施互不依赖，可以并行执行。建议执行顺序：**1+2 → 验证编译 → 3 → 验证编译 → 4（可选）**。

---

## 执行步骤

### Step 1：Release profile
1. 在 `Cargo.toml` 追加 `[profile.release]` 配置

### Step 2：Tauri features
1. 修改 `Cargo.toml` 中 `tauri` 依赖为 `default-features = false`

### Step 3：PTY 移除
1. 删除 `pty.rs`、`ptyCleanBridge.ts`、`ptyProfiles.ts`
2. 修改 `main.rs`、`commands.rs`、`Cargo.toml`
3. 清理前端 `types.ts`、`api.ts`、`App.tsx`、`SessionView.tsx`、`Composer.tsx`
4. 更新 `package.json`、`main.tsx`、`app.css`、`mockData.ts`
5. 全量编译验证

### Step 4：验证
- `cargo check`（Rust 编译）
- `npx tsc --noEmit`（TypeScript 编译）
- 人工验收：打开应用，确认正常交互（会话、发送消息、切换项目等）
