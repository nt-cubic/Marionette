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
| 项目 / 会话 | 项目 CRUD、搜索 threads（按标题过滤，不读正文）、项目拖拽排序、会话按发送 recency 排序；空闲 30 分钟的会话自动「挂起」（结束 agent 进程，下次发消息自动接上历史）；行内 ⏸ 只在进程还活着时画（starting / running / waiting / error），按下去就是停——正在跑的直接中断并结束进程，说出口的半截回复留着并标「已挂起」；已挂起的会话不画按钮；冷启动恢复上次焦点和分屏（无存档则不打开对话，点底部输入框会在 Chat 新建） |
| Chat 对话 | 侧栏项目列表下的独立对话列表；使用进程默认工作目录，不创建项目或工作区 `.marionette`；资源管理器「在此处打开」遇到已有 `.marionette` 才按项目打开 |
| 多 Agent | 内置 + custom ACP agents；懒启动、能力协商、model/mode/effort/文件权限（agent 自己 advertise 的旋钮才出现，按 option id 判定，category 只当提示） |
| Clean 对话 | transcript JSONL、edit&resend（重发会带上原来的 agent/模型标记，后面的回复仍是 Reply 而不是 Notification）、普通权限弹窗、Ask/Plan 进对话流（卡片上回复，输入框收起，写入 handoff.md）、@ 派任务、从任意一条回复分叉出新会话（复制到该条为止，首次发送自动带上这段历史）、回复里的图片按会话工作目录解析（文件名差一个字符时找同目录里唯一近似的那张）；Agent 读到的图片在工具卡里直接出预览；输入框贴图带缩略图；点图片开四周发黑的灯箱（拖动平移、滚轮缩放）；并行子代理（Grok `spawn_subagent` / Claude parentToolUseId / OpenCode Task）收进专用格子而不是刷满主消息流；Markdown 走主流 GFM（react-markdown + remark-gfm，编号列表 10/1000、重复字不再被流合并吃掉）；默认闭眼折叠 tool/thinking，进行中显示当前步骤，结束后摘要为 `1 think, 2 read, 3 edit`，开眼状态会记住 |
| Handoff / Git | handoff.md、changed files、只读 diff；**每轮交付卡走私有 index 的内容树快照（本轮窗口口径，见下节）** |
| 多窗口 | Tab 幽灵撕出 → 松手开 detached 窗；关窗 hide 防 tao 闪退；隐藏壳 LRU 回收。主窗对话区最多 4 格分屏（拖标签到中区/格的左、右、上、下约 1/4；可 1\|1\|1、1\|2 或 2×2；同一方向再拆会均分整行/整列）；每格自己的标签、转写、输入框；标签 Columns2 / 右键「在右侧打开」「在下方打开」；重启恢复分屏。撕出去的窗口拖回主窗：边缘拆格、中间并入、坐标失败进焦点格；关撕窗也插回焦点格。侧栏点已撕开的对话只聚焦那个窗口。权限/提问/计划卡片会把焦点切到当事格；权限框一到就画 |
| Usage | 上下文、OpenCode 余额探测、Claude/Codex/Grok 限流类数据、**订阅配额（引擎 `magpie quota`，跨 agent 可见的订阅窗口并入同一面板）** |
| 登录 | 多 agent probe + Sign in 横幅 |
| **Telegram 远程** | 左下角纸飞机图标：@BotFather 建个 bot 粘 token（getMe 校验，DPAPI 落 `~/.marionette/telegram.json`）→ 给 bot 发条消息在浮层确认配对（只认这一个 chat）。会话跑完/等你批准（权限、ask、plan）/出错/进程死亡推送到手机；**跑完推送带全文正文，MarkdownV2 格式（代码块/粗体/链接），长文自动分片**；权限/ask/plan 的通知带内联按钮，远程解答与 UI 共享同一 waiter（先答先得）；在 Telegram 里回复文字 = 送进对应会话（引用回复定向，裸文本去最近活跃会话），走与本地输入完全相同的发送管线（挂起重连/历史注入/排队）；权限等待 120s 到期前 30s 追加提醒。**置顶「总控」面板**：谁在跑/谁在等你实时重画（忙时每 60s 保鲜），按钮点会话名 = 切换裸发文字去向，⏹ = 远程停止（与 UI 停止键同路），`/status` 拉全量清单。通知偏好（跑完/等待/出错/带内容）在浮层开关。纯出站 HTTPS，零新依赖；未配对时零行为变化。详见 [`telegram-bridge.md`](./telegram-bridge.md) |
| **跨用（模型/key/订阅互通）** | 见下节 |

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
- **模型目录与显示管理**：统一按来源分组原生和共享模型，只有「接口主机 + 凭证」确认同一账号的入口才合并（agent 自带 provider、Grok 的 `[model.*]` 表、引擎目录会并成一组，标题用 agent 的厂商名）；订阅登录态（Codex 的 ChatGPT、Grok 的 xAI）同样只算一个账号——拥有登录文件的 agent 整份目录并入引擎分组，另一个 agent 复制的同一 OAuth 登录（OpenCode 的 `openai`）按 `accountId` 判重后不再单列；模型名相同但主机或凭证不同的条目保留。合并前保存的隐藏与排序按旧标识迁移。模型菜单底部的「配置模型与来源…」与模型共享浮层里的「模型与来源 · 显示和排序…」都打开这一页，可分别隐藏 provider / 模型、调整 provider / 组内模型顺序、恢复默认；大来源默认收起、排序箭头悬停才出现；保存到 `~/.marionette/model-library.json`，多个窗口同步，隐藏不影响已有会话。扫描 Grok / Pi / OpenCode / Codex / Claude 的用户自定义接口，保留原配置引用，经引擎供其他已接桥 agent 使用；扫描位置与失败原因可在凭据页查看。「添加密钥」支持自定义 Chat Completions / Responses / Anthropic 接口和显式模型 ID。范围与使用说明见 [模型与来源](model-library.md)。
- **卫生**：删除凭据条目会从引擎回收对应 provider（注册账本对账）；30 分钟无绑定且无需求的空闲引擎自动停 serve，任何跨用入口（菜单/绑定/按钮/启动）自动唤醒。
- 安全底线不变：明文 key 只进 env / owned-section / magpie CLI 命令行，绝不进日志与 agent 配置；扫描只读。

## 交付卡（每轮改动的口径）

卡上的文件列表 = **本轮窗口内的真实内容差异**，不是工作区状态。

- **基线**：`send` 时用私有 git index（复制用户 `.git/index` 当 stat 缓存 + 私有 object dir/alternates）给整个 worktree 拍一棵内容树（`git_service::begin_workspace_snapshot`，实测本仓库 0.6–1.2s；裸 `read-tree`+`add --all` 要 8.6s，所以必须 seed）。turn 末再拍一次，`diff-tree -r -M --name-status/--numstat` 出每文件增删——`+/-` 是真数字，不再靠工具 card 里的 diff 行数猜。
- **为什么不用 `git status` / `git diff HEAD`**：二者口径在本仓库实测分叉——`status -uall` 报 224~227 条，真实内容差异只有 52 条（`third_party/magpie/**` 这类 `text=auto`/CRLF 文件 stat-dirty 但 diff 0 行），于是「按 status 列文件、按 diff 算指纹」会给 186 个文件留空指纹，任何比对都可能把它们当成新改动。
- **谁算本轮的**：起点干净、结束脏 → 无条件算（工具、子代理、`npm run build` 都认）；**起点就脏**的文件只有被本轮自己的 edit/write 工具碰过才算，否则同一个工作区里另一场对话（或用户的编辑器）的改动会串进这张卡。没有 HEAD 时无法拆分，宁可全留。
- **fail closed**：基线拍失败/不是 git 仓库 → 不发布任何 git 真相，卡片只留本轮工具账本。旧实现 `catch { return [] }` 把失败当「基线是空的」，于是整库脏文件全进了卡——这就是「交付卡显示全仓库改动」的直接成因（2026-10-23 修）。
- 子目录项目（项目根 ≠ 仓库根）的路径按**项目根**相对返回（`rev-parse --show-prefix` 剥掉），与 `getFileDiff` 的解析口径一致。
- **一行一个文件**：卡片合并的键是规范化路径（正斜杠、去掉 cwd 前缀、小写），不是原始字符串——git 报的是工作区相对路径（`src/lib/api.ts`），工具调用里带的是 agent 原样给的绝对路径（`D:\Myself\Marionette\src\lib\api.ts`），按原串合并会把每个被工具改过的文件列两遍（2026-10-23 修，回归用例在 `scripts/verify-turn-delivery.mts`）。git 报出的相对路径优先用于行标题、diff 请求与撤销。

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
