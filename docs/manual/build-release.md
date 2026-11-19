# 构建与发布手册（build-release）

一句话：从开发运行到发布 exe 的全部命令、双 SKU 机制与安装器。

## 文件地图

| 想做什么 | 去哪 | 备注 |
|---|---|---|
| 开发运行 | 仓库根 `npm run tauri dev` / `start-marionette.bat` | |
| 前端构建 | `npm run build`（`tsc && vite build`） | |
| Rust 检查/测试 | **`cmd /c check.bat …`**（自动带 MSVC 环境） | 裸 cargo 撞 `cl.exe not found` |
| 便携版构建 | `tools\build-portable.bat` | 默认 Rust 进程内引擎 |
| magpie 回退构建 | `set MARIONETTE_ENGINE=magpie && tools\build-portable.bat` | 一个发布周期回退；走导入/种子 go build/载荷 UPX |
| 引擎种子导入 | `tools/import-magpie.ps1` | 仅 magpie 回退；`SKIP_MAGPIE_IMPORT=1` 跳过；`MAGPIE_TAG=v…` 钉版本 |
| 版本号 | `tools\bump-version.ps1`（或 .bat） | |
| 发布 | `tools\publish-release.bat` | 根目录 `install.ps1` 一并作 Release 资产上传 |
| 一行安装器 | 根 `install.ps1` | `irm …/install.ps1 | iex`；LOCALAPPDATA+开始菜单+PATH；重跑升级；`-Uninstall` 卸载 |
| 本地开发装机 | `tools\install-app.ps1` | 拷 `release\current-build.txt` 指向的产物；与 install.ps1 逻辑保持同步 |
| 演示实例 | `tools\demo-instance.bat` | 隔离 profile；抓图禁忌见 `tools/README.md` |
| 合同套件 | `scripts/verify-engine-contract.mts` | 唯一验收尺 |
| 真机 e2e | `scripts/verify-rust-sku-e2e.mjs` | 隔离 profile + CDP 9223，**别混 --lang** |

## 双 SKU 机制（build-portable.bat）

- **默认（不设变量）= Rust 进程内引擎**：Cargo 默认 feature 为 `engine-rust`，
  整段 magpie 准备跳过（等价 :prepare_embed），产物照旧 canonical
  `release\<版本>\Marionette.exe`。体积 ~5.36MB（UPX --best --lzma 后）。
- **`MARIONETTE_ENGINE=magpie` = 一个发布周期的回退 SKU**：import-magpie.ps1
  拉上游 → 种子 go build（`-tags "nogui marionette_seed"`）→ UPX → 内嵌载荷。
  产物 ~25MB；验证稳定后移除。
- **两 SKU 共用**：canonical 产物路径、`(N)` 降级机制（同版本在跑时 canonical 被锁 →
  写 `Marionette (N).exe` 并算成功；`current-build.txt`/快捷方式不动；**发布只认
  canonical，别拿 (N) 发**）。
- **SKU 选择**：Cargo 默认启用 `engine-rust`；`build.rs` 在看到
  `MARIONETTE_ENGINE=magpie` 或 `embedded-engine` 时派生 `engine_rust_active=false`，
  自动让位给 Magpie。Tauri CLI 没有 `--no-default-features`，别再引入依赖它的构建命令。

### 三态编译检查单（动 Cargo.toml/features 后必跑）
```bash
cmd /c "check.bat check"                                # 默认 Rust 进程内引擎
cmd /c "check.bat check --no-default-features"           # magpie 下载/复用回退路径编译
# 回退路径由 build.rs 的 engine_rust_active 覆盖；不要恢复 Cargo feature compile_error
```

## 发布流程

1. `tools\bump-version.ps1` 出版本号；
2. `tools\build-portable.bat` → `release\<版本号>\Marionette.exe`；
3. `tools\publish-release.bat`（install.ps1 随上传）。

## 坑

- **Low 完整性标签**（esbuild Access is denied / exe 双击没反应）→
  见 [debug-runbook.md](debug-runbook.md)，跑 `scripts\fix-workspace-integrity.ps1`。
- rust 态 `place_and_prep` 会补建引擎目录（注册账本 warn 的根因修复）——别删。
- 引擎种子精简版：`tools/magpie-seed/main_seed.go`（build tag `marionette_seed`）绕过
  internal/cli 只挂 gateway/routing/usage；上游重构 serve 路径 = 编译报错（响亮）。
  Rust 已是默认 SKU；种子只服务 `MARIONETTE_ENGINE=magpie` 回退。
- `engine-rust` 态 check_update/update/restore_factory 走短路径（前端更新面板不卡、
  按钮 自然隐藏）——别在 rust 态接 magpie 更新链。

## 已知差距与源流

- 蒸馏自：AGENTS.md「常用命令/目录」+ plan-engine-rust.md T12/T13 段 +
  build-portable.bat 的 ENGINE_SKU 分支实读。
- 工具脚本清单与参数：`tools/README.md`；验证脚本清单：`scripts/README.md`。
