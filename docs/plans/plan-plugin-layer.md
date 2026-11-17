# 共通插件层（Universal Plugin Layer）设计稿

日期：2026-11-05 落地。行为速览见 `docs/CURRENT.md`「共通插件层」；红线另见 `AGENTS.md` 同名节。

## 要解决的问题

「安装一个插件，所有 agent 都生效」。此前这个能力散在五条互不相通的管道上：
`context_inventory` 借贷（按项目 opt-in、MCP 默认关、stdio 一条就能毒死 zcode 的
session/new）、`zcode_mcp_bridge`（zcode 专属 stdio 旁路）、`tools_mcp`（只装自家
23 个工具）、`dsh_plugins`（DSH 专属；**2026-11-06 已删**，见落地偏差）、skills
prompt 借贷（只扫各 agent 自家目录）。
Marionette 没有「插件」的一等概念——MCP 清单只是对 agent 配置文件的扫描。

## 业界对标（2026-11-04 调研）

**没有任何产品做成了运行时统一层**：

- **Codeg**（同赛道）：市场安装 + **写回 19 家 CLI 各自配置**（`commands/mcp.rs`
  10312 行 per-agent 特判），自家调研承认「逐 agent 验证，没有通用规律」（OpenClaw
  拒收任何条目、Cline 收下不启动、pi-acp 收了就丢、双注册一堆）；其工具通道（stdio
  伴生进程 + named pipe）在 zcode 上直接不可用。唯一真跑通的是 **skills junction**
  （中央仓 + 链接进各 agent 技能目录 + 矩阵 UI）——文件级、无 schema、值得后续借鉴。
- **Zed**：`session/new` 清单直传（与我们借贷同路线），踩同样的 capability 坑
  （extension MCP 不转发 #52449、工具静默丢失 #55642、「宿主配置 + agent 原生配置」
  双源困惑）。
- **VS Code**：`.vscode/mcp.json` 只转发给 Copilot harness；Claude Code / Codex
  harness 官方明文各用各的原生配置。
- **单 agent 工具**（Claude Code plugins / OpenCode plugins / Codex / Gemini /
  Cline）：插件 = 给自己的 MCP+skills+hooks 包，无跨宿主复用先例。
- **mcpd（Mozilla）/ MCPHub / mcphub.nvim**：独立 localhost 网关 daemon——与
  `tools_mcp` 同型、方向被验证，但无一宿主内置。
- **ACP v2**：类型收敛 stdio/http + capability 声明；**明文鼓励客户端用自己的
  MCP server 供工具**；安全条款要求 env/header 值不得进日志/错误/回放——与本层
  红线同向。

### 对 plan-codeg-borrow.md 的修订

本层**推翻 P2.2 的「写回 agent 配置」半边**：写回只对「agent 在 Marionette 之外
的原生使用」有价值，对「Marionette 会话内全 agent 生效」是绕远路（codeg 的万行
per-agent 矩阵就是代价）。P2.2 的市场安装（registry/Smithery）与 P2.3 的 junction
skills + 矩阵记为后续项。

## 架构

```
登记处 plugins.rs ── ~/.marionette/plugins/index.json（fail-open 读、原子写）
      │ 启用插件
      ▼
注入 acp.rs session/new ── 每个启用插件一条 url 型条目（与 marionette-tools 同形）
      │                    !supports_http 且收 stdio 的 agent → 回退原始 stdio spec
      ▼
宿主 plugin_host.rs ── 挂在 tools_mcp 的 localhost server 上：/mcp/p/<id>
      ├── stdio 插件：Marionette 按会话懒起子进程（MCP stdio ⇄ HTTP 翻译）
      │   resolve_spawn_command（npx/.cmd）+ CREATE_NO_WINDOW + Job Object kill-on-close
      └── http 插件：纯 HTTP 中继（真实凭据只留 Marionette，agent 只拿
          loopback URL + 会话 token；远端 Mcp-Session-Id 由宿主代持）
```

- **token**：复用 `tools_mcp` 的铸造与 token→session 映射（`register_session_token`），
  与浏览器工具总开关解耦（工具关掉插件照常工作）。
- **懒握手**：子进程在该会话首个请求到达时才 spawn + initialize；agent 的
  `initialize` 原样转发（protocolVersion/clientInfo 用 agent 的）。
- **生命周期**：`plugin_host::release_session` 挂在 acp.rs 的**两个** teardown 点
  ——`stop()`（显式停止）与 `read_stdout` EOF（agent 自崩）；禁用/卸载跨会话杀
  （`kill_plugin`）。进程崩溃由 Job Object 兜底。
- **撞名**：借贷条目与启用插件同名时借贷让位（插件赢），防 agent 内重名双注册。
- **skills**：`~/.marionette/plugins/skills/` 作为全局源进 `context_inventory::scan`
  （default-on，与其它 skills 同走 prompt 借贷）——装一次、全 agent 见。
  **2026-11-06 起可从界面装**：插件卡「技能」块（`UniversalSkillsSection`）从
  文件夹安装/停用/删除；登记处就是目录本身（含 SKILL.md 的子目录，不进
  index.json），全局停用 = 目录内 `.disabled` 标记文件（`scan_skills_dir` 跳过
  带标记的目录，「项目上下文→技能」与注入同步消失）。
- **协议边界**：child→agent 的 server 主动请求（sampling/roots）一律 -32601
  （agent 在 HTTP 请求边界后面，没法应答）；远端中继只收 JSON（`Accept:
  application/json`，流式 only 的服务会响亮地 406）。
- **UI**：ContextPanel 新 `UniversalPluginsSection`（自取数；安装表单 stdio =
  command/args/env、http = url/headers；探活/启停/卸载）；Ctrl K「共通插件」
  展开右栏滚到卡片、贴边态切 context 整页。i18n 片段 `fe-allplugins`。

## 红线（实现时踩过/规避的）

1. **不写任何 agent 自有配置**——扇出 writer 就是 codeg 那 1 万行的病。
2. **密钥不外流**：env/header 值不进 debug_log（只记方法名/计数）、不进 url 型
   注入条目（唯一凭据是 loopback 会话 token）；stdio 回退条目含 env 值，与 stdio
   借贷同姿态（agent 进程自己拉起）。
3. **持 CHILDREN 锁绝不等待子进程**（HelperIo 自锁教训）：spawn（含 where.exe
   解析）在锁外，锁内只做查表/插入；等待全走 pending channel。
4. **插件的 JSON-RPC error 原样透传**；只有宿主侧失败（spawn/超时/进程退出）才
   编 -32000。
5. `release_session` 两个挂点成对（stop + EOF），漏一个就漏杀子进程。

## 落地偏差记录

- 原计划「复用 acp_transport 的 request/notify」：它们绑死 `AcpProcess`，实际只
  复用了泛型的 `spawn_stdin_writer` 与 `json_rpc_id_as_u64`（可见性放宽到
  pub(crate)），request 通道按同形状在 plugin_host 内实现。
- 原计划 M1（直达注入）先行：实际一次落到位（登记处 + proxy + 注入），zcode 的
  stdio 借贷桥原样保留服务旧借贷，插件层不碰它——zcode 的 stdio 插件直接走 url
  型代理，不再需要桥。
- 测试内存管道：`std::io::duplex` 是 nightly-only，测试里写了通道版
  （`ChannelWriter/ChannelReader`，字节级，EOF 语义与真管道一致）。
- `http_client` 新增 `agent_with_timeout(timeout, follow_proxy)`：ureq 只能在
  agent 构建期设超时，控制面 15s / 工具调用 300s / 通知中继 10s 各建各的。
- inventory-zh `--check` 在本批前后均为同一预存失败（18 文件 120 行，与 HEAD
  持平），未新增。
- **2026-11-06 收口批**：右栏「DeepSeek Harness」插件块（`PluginsSection` +
  `pluginBadges` + Rust `dsh_plugins.rs` 三命令）整体删除——统一入口只剩
  「所有 agent」MCP 块与「技能」块；Ctrl K「插件」改为派发
  `marionette:reveal-plugins` 事件（ContextPanel 展开共享卡并滚动，修「折叠态
  滚不到」）。dsh 跨用桥与 spawn 链一字未动。skills 安装的复制树复用
  `memos::copy_tree`、frontmatter 解析复用 `context_inventory::
  parse_skill_front_matter`（均放宽 pub(crate)）。

## 回归

- `cargo test --bin marionette plugins` + `cargo test --bin marionette plugin_host`
  + `cargo test --bin marionette tools_mcp zcode_mcp_bridge`（桥不动）
- `npx tsc --noEmit`；`npx tsx scripts/inventory-zh.mts --check`（与 HEAD 持平）
- 手工 E2E：装 stdio 插件（`node -e` 单行 MCP server 或
  `npx -y @modelcontextprotocol/server-everything`）→ claude-code 与 **zcode**
  两边会话都应出现 `mcp__<id>__*` 并能调用 → 关会话任务管理器确认子进程被杀
  → 装 http 插件验证中继（agent 侧看不到真实 header）。
