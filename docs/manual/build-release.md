# 构建与发布手册（build-release）

一句话：从开发运行到发布 exe 的全部命令、双 SKU 机制与安装器。

## 文件地图

| 想做什么 | 去哪 | 备注 |
|---|---|---|
| 开发运行 | 仓库根 `npm run tauri dev` / `start-marionette.bat` | |
| 前端构建 | `npm run build`（`tsc && vite build`） | |
| Rust 检查/测试 | **`cmd /c check.bat …`**（自动带 MSVC 环境） | 裸 cargo 撞 `cl.exe not found` |
| 便携版构建 | `tools\build-portable.bat` | 默认 magpie SKU |
| rust SKU 构建 | `set MARIONETTE_ENGINE=rust && tools\build-portable.bat` | 跳过 magpie 导入/种子 go build/载荷 UPX |
| 引擎种子导入 | `tools/import-magpie.ps1` | `SKIP_MAGPIE_IMPORT=1` 跳过；`MAGPIE_TAG=v…` 钉版本 |
| 版本号 | `tools\bump-version.ps1`（或 .bat） | |
| 发布 | `tools\publish-release.bat` | 根目录 `install.ps1` 一并作 Release 资产上传 |
| 一行安装器 | 根 `install.ps1` | `irm …/install.ps1 | iex`；LOCALAPPDATA+开始菜单+PATH；重跑升级；`-Uninstall` 卸载 |
| 本地开发装机 | `tools\install-app.ps1` | 拷 `release\current-build.txt` 指向的产物；与 install.ps1 逻辑保持同步 |
| 演示实例 | `tools\demo-instance.bat` | 隔离 profile；抓图禁忌见 `tools/README.md` |
| 合同套件 | `scripts/verify-engine-contract.mts` | 唯一验收尺 |
| 真机 e2e | `scripts/verify-rust-sku-e2e.mjs` | 隔离 profile + CDP 9223，**别混 --lang** |

## 双 SKU 机制（build-portable.bat）

- **默认（不设变量）= magpie SKU**：import-magpie.ps1 拉上游 → 种子 go build
  （`-tags "nogui marionette_seed"`）→ UPX → 内嵌载荷。产物 ~25MB。
- **`MARIONETTE_ENGINE=rust` = rust SKU**：整段 magpie 准备跳过（等价 :prepare_embed），
  cargo features 换 `engine-rust`，产物照旧 canonical `release\<版本>\Marionette.exe`。
  体积 ~5.36MB（UPX --best --lzma 后）。尾部打 SKU 标注 + 体积。
- **两 SKU 共用**：canonical 产物路径、`(N)` 降级机制（同版本在跑时 canonical 被锁 →
  写 `Marionette (N).exe` 并算成功；`current-build.txt`/快捷方式不动；**发布只认
  canonical，别拿 (N) 发**）。
- **feature 互斥**：`engine-rust` 与 `embedded-engine` 同开 = engine.rs 顶部
  compile_error!（Cargo 语法表达不了互斥）。

### 三态编译检查单（动 Cargo.toml/features 后必跑）
```bash
cmd /c "check.bat check"                                # 默认（magpie SKU）
cmd /c "check.bat check --features engine-rust"         # rust SKU
# 双 feature 同开：编译期 compile_error 报错 = 预期行为
```

## 发布流程

1. `tools\bump-version.ps1` 出版本号；
2. `tools\build-portable.bat`（或 rust SKU 变体）→ `release\<版本号>\Marionette.exe`；
3. `tools\publish-release.bat`（install.ps1 随上传）。

## 坑

- **Low 完整性标签**（esbuild Access is denied / exe 双击没反应）→
  见 [debug-runbook.md](debug-runbook.md)，跑 `scripts\fix-workspace-integrity.ps1`。
- rust 态 `place_and_prep` 会补建引擎目录（注册账本 warn 的根因修复）——别删。
- 引擎种子精简版：`tools/magpie-seed/main_seed.go`（build tag `marionette_seed`）绕过
  internal/cli 只挂 gateway/routing/usage；上游重构 serve 路径 = 编译报错（响亮）。
  rust SKU 落地后种子只服务 magpie SKU；T14 退役决策（默认 SKU 翻转）待产品定。
- `engine-rust` 态 check_update/update/restore_factory 走短路径（前端更新面板不卡、
  按钮 自然隐藏）——别在 rust 态接 magpie 更新链。

## 已知差距与源流

- 蒸馏自：AGENTS.md「常用命令/目录」+ plan-engine-rust.md T12/T13 段 +
  build-portable.bat 的 ENGINE_SKU 分支实读。
- 工具脚本清单与参数：`tools/README.md`；验证脚本清单：`scripts/README.md`。
