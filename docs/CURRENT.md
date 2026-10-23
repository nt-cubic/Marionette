# Marionette — 当前状态（2026-10）

> 现行产品说明。历史设计稿与已完成路线图见 [`archive/`](./archive/)。

## 一句话

Windows 本地 **多 Agent CLI 图形壳**（Tauri + React）。不替代 OpenCode / Claude / Codex / Grok 等原厂 agent，只负责启动、展示、切换、记录与 handoff——外加把「任何一个来源的模型 / key / 订阅」喂给「任何一个 agent」的**跨用引擎**。

## 产品底线（仍有效）

- 不做自研 Agent runtime / 模型路由器 / 云同步 / MCP marketplace
- ACP 优先；会话绑定 agent；Clean 为主界面
- 轻、快；不做成 IDE 或 API 管理平台

## 已具备（相对用户可感知）

| 能力 | 说明 |
|------|------|
| 项目 / 会话 | 项目 CRUD、搜索 threads、项目拖拽排序、会话按发送 recency 排序；空闲 30 分钟的会话自动「挂起」（结束 agent 进程，下次发消息自动接上历史）；行内 ⏸ 只在进程还活着时画（starting / running / waiting / error），按下去就是停——正在跑的直接中断并结束进程，说出口的半截回复留着并标「已挂起」；已挂起的会话不画按钮；冷启动恢复上次焦点和分屏（无存档则不打开对话，点底部输入框会在 Chat 新建） |
| Chat 对话 | 侧栏项目列表下的独立对话列表；使用进程默认工作目录，不创建项目或工作区 `.marionette`；资源管理器「在此处打开」遇到已有 `.marionette` 才按项目打开 |
| 多 Agent | 内置 + custom ACP agents；懒启动、能力协商、model/mode/effort/文件权限（agent 自己 advertise 的旋钮才出现，按 option id 判定，category 只当提示） |
| Clean 对话 | transcript JSONL、edit&resend（重发会带上原来的 agent/模型标记，后面的回复仍是 Reply 而不是 Notification）、普通权限弹窗、Ask/Plan 进对话流（卡片上回复，输入框收起，写入 handoff.md）、@ 派任务、从任意一条回复分叉出新会话（复制到该条为止，首次发送自动带上这段历史）、回复里的图片按会话工作目录解析（文件名差一个字符时找同目录里唯一近似的那张）；Agent 读到的图片在工具卡里直接出预览；输入框贴图带缩略图；点图片开四周发黑的灯箱（拖动平移、滚轮缩放）；并行子代理（Grok `spawn_subagent` / Claude parentToolUseId / OpenCode Task）收进专用格子而不是刷满主消息流；Markdown 走主流 GFM（react-markdown + remark-gfm，编号列表 10/1000、重复字不再被流合并吃掉）；默认闭眼折叠 tool/thinking，进行中显示当前步骤，结束后摘要为 `1 think, 2 read, 3 edit`，开眼状态会记住 |
| Handoff / Git | handoff.md、changed files、只读 diff |
| 多窗口 | Tab 幽灵撕出 → 松手开 detached 窗；关窗 hide 防 tao 闪退；隐藏壳 LRU 回收。主窗对话区最多 4 格分屏（拖标签到中区/格的左、右、上、下约 1/4；可 1\|1\|1、1\|2 或 2×2；同一方向再拆会均分整行/整列）；每格自己的标签、转写、输入框；标签 Columns2 / 右键「在右侧打开」「在下方打开」；重启恢复分屏。撕出去的窗口拖回主窗：边缘拆格、中间并入、坐标失败进焦点格；关撕窗也插回焦点格。侧栏点已撕开的对话只聚焦那个窗口。权限/提问/计划卡片会把焦点切到当事格；权限框一到就画 |
| Usage | 上下文、OpenCode 余额探测、Claude/Codex/Grok 限流类数据、**订阅配额（引擎 `magpie quota`，跨 agent 可见的订阅窗口并入同一面板）** |
| 登录 | 多 agent probe + Sign in 横幅 |
| **Telegram 远程** | 左下角纸飞机图标：@BotFather 建个 bot 粘 token（getMe 校验，DPAPI 落 `~/.marionette/telegram.json`）→ 给 bot 发条消息在浮层确认配对（只认这一个 chat）。会话跑完/等你批准（权限、ask、plan）/出错/进程死亡推送到手机；**跑完推送带全文正文，MarkdownV2 格式（代码块/粗体/链接），长文自动分片**；权限/ask/plan 的通知带内联按钮，远程解答与 UI 共享同一 waiter（先答先得）；在 Telegram 里回复文字 = 送进对应会话（引用回复定向，裸文本去最近活跃会话），走与本地输入完全相同的发送管线（挂起重连/历史注入/排队）；权限等待 120s 到期前 30s 追加提醒。**置顶「总控」面板**：谁在跑/谁在等你实时重画（忙时每 60s 保鲜），按钮点会话名 = 切换裸发文字去向，⏹ = 远程停止（与 UI 停止键同路），`/status` 拉全量清单。通知偏好（跑完/等待/出错/带内容）在浮层开关。纯出站 HTTPS，零新依赖；未配对时零行为变化。详见 [`telegram-bridge.md`](./telegram-bridge.md) |
| **跨用（模型/key/订阅互通）** | 见下节 |
| **悬浮球（后台常驻助手，2026-10-23）** | 桌面右下角常驻**原生 Win32 圆盘**（`orb_native.rs`：提线木偶标 + Zed 色板、分层置顶、托盘锚点）。左键在身边弹出卫星：记（Memo 时间流）、额（配额圆环上伸）、+（新对话）、运行中（有任务时）。额卫星把同一窗口拉成 **QuotaBar ProviderRing** 栈（每家一圈：中间品牌 logo、弧长=剩余、颜色走 UsageRamp 用尽色阶）。记卫星打开主窗 **Memo** 页（收集箱 user_message 时间线：顶栏速记 + 按日卡片）。文件仍可丢到球上，剪贴板走右键菜单。关主窗 = 隐藏到后台（默认开）；退出只从球或托盘菜单进。 |

## 跨用引擎（2026-10 定稿形态）

目标：**用户无脑打开软件就能用**——任何一个来源的模型，任何一个 agent 的会话里直接选。

- **key 型直连**（OpenCode Go / DeepSeek / GLM / Kimi / OpenRouter / xAI）：claude-code 走 spawn env、grok 走 owned 配置表，不过网关；来源含凭据库手存的 key 和 opencode `auth.json` 的只读扫描引用（引用只在直连注入瞬间读钥匙，不复制）。
- **订阅型 + codex 全场景**：经内嵌翻译引擎（[yetone/magpie](https://github.com/yetone/magpie)，MIT，Go）。引擎**按需唤醒**：打开跨用菜单（最多等 5s）、绑定共享模型（最多等 20s）、启动已接桥的 agent 会话都会自动拉起；30 分钟无绑定且无需求自动停 serve。key-plan 与扫描引用的 api key 自动注册进引擎；Grok / ChatGPT 订阅由引擎 discovery 自己发现。codex 只接引擎源（协议约束），经 `providers/set` 改道 + 路由组把内建模型名别名到所选引擎模型。
- **DSH（@openma/deepseek-harness-acp）跨用（2026-10-20）**：spawn 时把 llm-pi-ai 托管路由 `marionette`（openai-completions → 引擎 `/v1`）写进 `~/.dsh/cordis.patch.yml`（文本外科手术、幂等、保用户内容），令牌常量进 `~/.dsh/.credentials.yaml` refs + spawn env；模型经 dsh-acp 原生目录广告（id `marionette::provider/model`）。引擎停用即移除路由；home patch 被 dsh Web/desktop 共享，Web 里也能走这条路由。写入器在 `dsh_bridge.rs`，调研定稿见 `research-dsh-cross-use.md` §8。
- **pi（pi-acp）跨用（2026-10-20）**：spawn 时把 provider `marionette` 合并进 `~/.pi/agent/models.json`（pi 官方兼容端点机制，`apiKey` 用 `$MARIONETTE_ENGINE_TOKEN` env 插值、凭据不落盘；语义幂等、保用户 provider）；模型经 pi-acp 原生目录广告（id `marionette/<engineId>`，与 grok/opencode 前缀同形），`set_config_option` 直接切。引擎停用即移除托管段。写入器在 `model_bridge.rs`（`refresh_pi_marionette_route`）。
- **Roster 收束（2026-10-20）**：内置 agent 收为「五跨用（claude-code / grok / codex / opencode / deepseek）+ **pi + hermes**」；cline / gemini / kimi / codebuddy / openclaw / cursor / omp 退出内置名单（cursor 封闭无口子、gemini 协议锁死，其余用户面小——可接性矩阵见 `research-acp-cross-use-matrix.md`）。退出者可经「自定义 agent」随时加回；历史会话导入的 parsers 与登录探针保留未动。hermes 的跨用桥待装好 hermes 后按矩阵处方接（config.yaml providers，双协议面）。
- **一种便携版**（`tools\build-portable.bat` → `release\<版本号>\Marionette.exe`，~16MB）：magpie 打进 exe 作出厂种子，打开即离线落位。打包默认先拉上游 **latest 稳定版**（`import-magpie.ps1`；`SKIP_MAGPIE_IMPORT=1` / `MAGPIE_TAG=` 可跳过或钉版本）。发布用 `tools\publish-release.bat`。
- **引擎可更新**：本机权威在 `~/.marionette/engine/`。左下角「模型共享」可检查/下载 yetone/magpie 稳定版（约 48MB CLI），下完即粘住、启动不再被出厂种子盖回；「恢复出厂引擎」才再解 exe 内嵌。与 Harness 等 agent 自更新同级，不必等 Marionette 发版。
- **入口**：左下角互联图标 = 模型共享状态/一键启动；凭据管理退居其浮层底部链接。用量面板点刷新会拉网关 `/v1/magpie/quotas`（各家订阅额度）。
- **模型菜单**：原生模型 + 按来源分组的共享段（每组右上角「共享」标记：Grok 订阅 / OpenCode Go / DeepSeek …）。
- **卫生**：删除凭据条目会从引擎回收对应 provider（注册账本对账）；30 分钟无绑定且无需求的空闲引擎自动停 serve，任何跨用入口（菜单/绑定/按钮/启动）自动唤醒。
- 安全底线不变：明文 key 只进 env / owned-section / magpie CLI 命令行，绝不进日志与 agent 配置；扫描只读。

## 明确不做 / 暂缓

- 内嵌 llama.cpp / 本地推理 runtime
- 完整文件树 / IDE 级编辑器
- 全 Rust UI 重写
- 把历史 ASP 文档当现行协议实现
- 裸 CLI 全局配置接管（终端里裸跑 claude/codex 不经 Marionette 也能用引擎——已明确不做，见归档设计稿）

## 工程入口

- 前端：`src/`
- 后端：`src-tauri/src/`
- 启动：`start-marionette.bat`（根目录唯一入口）/ `npm run tauri dev`
- 便携构建：`tools/build-portable.bat` → `release/<版本号>/Marionette.exe`
- 引擎源码吸纳/升级：`tools\import-magpie.ps1`（默认 latest 稳定；或 `-Tag <上游tag>`）
- 验证脚本：`scripts/`；Windows 脚本：`tools/`（各目录索引见其中的 `README.md`）

## 归档说明

`docs/archive/` 保留早期策划、路线图与 superpowers 规格，便于考古，**不以之为当前实现契约**。有冲突时以代码与本文件为准。跨用引擎的完整设计稿（M0-M3 过程记录）已归档至 [`archive/cross-agent-bridge.md`](./archive/cross-agent-bridge.md)。
