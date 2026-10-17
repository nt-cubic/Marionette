# Marionette

**你 -> Marionette -> 所有 Agents Harness**

鞋子每天在变，但人没有理由削足适履。

使用提线木偶（Marionette），让它帮你操作所有 Agents。

OpenCode、Claude Code、Grok Build、Codex、Pi、DeepSeek……把他们配进同一个壳里。

让所有 Agents 坐在一起，共享同样的知识和经验——**任意来源的模型、钥匙、订阅，也能喂给任意一个 Agent。**

你不用适应工具，让工具来适应你不断精进的做事方法。

在即将到来的时代，让 AI 成为你的部下，开始真正的运筹帷幄。

![主界面](docs/screenshots/main.png)

> 当前版本 **v0.3.1** · [Releases](https://github.com/nt-cubic/Marionette/releases/tag/v0.3.1)
>
> 普通版约 2 MB（引擎用时再下）· 内置引擎版约 16 MB（离线也能跨用）

**现行状态说明：** [docs/CURRENT.md](docs/CURRENT.md)  
（历史策划/路线图已挪到 [docs/archive/](docs/archive/)）

---

## 为什么存在 Marionette

Agents 更新 + 定价变化 + 用量限制 = 总是需要换用 Agents。  
模型、钥匙、订阅也一样：今天走 Grok，明天走 OpenCode Go，后天把同一把 key 喂给 Claude。

Marionette 让你在同一个工具里操纵所有 Agents，也让**同一个来源**出现在每一个会话的模型菜单里。

关注你作为人，使用 AI 的能力，而不是让人来适应持续变化的 Agents。

---

## 五个核心能力

### 一个窗口，所有 Agent

Claude Code、OpenCode、Codex、Grok Build、Pi、DeepSeek Harness、Cline、Gemini、Kimi、CodeBuddy、Hermes、OpenClaw、Cursor、OMP。

一个窗口，全部装下。自定义 ACP 也能加进来。

按需启动——不输入，不运行，不占资源。

### 换人，不换脑子

切换 Agent，上下文不会断。

Marionette 自动生成一个普通的 Markdown 文件，让不同的 Agent 可以根据此资料进行对话接力。

命令行里聊过的，点项目旁的刷新就能收进列表，按时间排在一起，点开接着聊。只收你添加过的目录。

### 一套配置，全线打通

MCP、Skills —— 不需要每个 agent 都各自配置。

扫描一次，统一管理，全部共享。

同一个工具，OpenCode 能用，Claude Code、Codex、Grok 也能用。

### 跨用：一个来源，喂给任何一个 Agent

打开软件就会把跨用网关拉起来，不用先去「配密钥」才能看见模型。

- **直连 key**（OpenCode Go / DeepSeek / GLM / Kimi / OpenRouter / xAI）：写进凭据，或本机已有 OpenCode 钥匙，Claude / Grok 会话里直接能选
- **订阅**（Grok、ChatGPT）以及 **Codex**：走内置翻译引擎。Codex 把内建模型名接到你选的那一个上
- **OpenCode** 同一套动作：左下角互联起来，模型菜单下次打开就有

左下角互联图标 = 网关状态 / 一键启动 / 停止。密钥管理在浮层底部。  
模型菜单按来源分组；同一模型优先走直连。闲置会自己停，再点就会醒。

### 轻，而且有两档

打开就工作，关闭就消失。单文件免安装。

| | 普通版 | 内置引擎版 |
|---|---|---|
| 体积 | 约 2 MB | 约 16 MB |
| 引擎 | 第一次跨用再下载 | 打进 exe，离线也能开跨用 |
| 适合 | 日常、磁盘紧 | 不想等下载、要随身带齐 |

腾出更多的空间打开工作与娱乐，让本地模型跑的更轻松。

---

## 还有更多

- **消息卡片** —— 所有 Agent 输出统一渲染，思考过程折叠，工具调用清晰
- **工具卡片** —— agent 读过的图片直接显示，编辑类工具给出带行号的 diff，可逐行批注
- **对话目录** —— 无需上下求索，直接罗列你曾说过的话
- **图片批注** —— 无需与 AI 隔空对话，直接点击与框选图片位置，直指要改的地方
- **对话标记** —— 选中消息添加批注，下次打开还在，可带入 Composer 引用
- **每一笔消耗，都摆在桌上** —— 余额、额度、速率限制、订阅配额，右侧栏实时刷新
- **持久化** —— 所有会话存 JSONL，全文可搜索，重开不丢；命令行历史刷新后也落在同一套列表里
- **Git 感知** —— 项目文件变化自动检测，一眼看到改了什么
- **@ 委派** —— 消息里 @ 另一个 Agent 派发任务，上下文自动交接
- **智能回复** —— 文件刚拖进来、AI 给了选项、上轮报错、让它继续执行……直接智能感知出现回复气泡，一键接话
- **聊天** —— 侧栏另有一组独立对话：不挂在任何项目下，用进程当前目录，也不会随手留下 `.marionette`
- **Tasks 任务清单** —— 项目级 Todo，手写增删、换 Agent 不丢，AI 的 Plan 一键吸收
- **多窗口** —— 拖出 tab 用幽灵预览，松手再开独立窗；项目可拖排序
- **置顶** —— 项目与会话都能钉在列表顶部，置顶不打乱「最近发送」排序
- **资源管理器** —— 侧栏打开项目文件夹

---

## 快速上手

```bash
git clone https://github.com/nt-cubic/Marionette
cd Marionette
npm install && npm run tauri dev
```

添加项目 → 选择 Agent → 开始对话。

> 目前只支持 Windows（需要 WebView2 Runtime，Win11 与更新过的 Win10 自带）。
> Windows 上也可以直接双击根目录的 `start-marionette.bat`：它会先检查 Node / Rust / MSVC，缺了会引导你安装。

### 直接下载

[Releases](https://github.com/nt-cubic/Marionette/releases/latest) 里两个 exe，选一个双击即可：

```bash
# 普通版（约 2 MB）
gh release download --clobber -p "Marionette-[0-9]*-windows-x64.exe"

# 内置引擎版（约 16 MB，离线也能跨用）
gh release download --clobber -p "Marionette-Engine-*-windows-x64.exe"
```

---

## 仓库结构

```
Marionette/
├─ start-marionette.bat   # 唯一双击入口：起开发版
├─ src/                   # 前端（React）：app / components / lib / styles
├─ src-tauri/             # 后端（Rust + Tauri 2）
├─ third_party/magpie     # 跨用引擎源码（yetone/magpie，内置引擎版编进 exe）
├─ docs/                  # 文档：CURRENT.md 是现行契约，索引见 docs/README.md
├─ scripts/               # 代码验证脚本（caps / stream / usage 冒烟）
└─ tools/                 # Windows 启动、构建、发布脚本 + 本地二进制（upx）
```

约定：根目录只放工具链需要的文件（`package.json`、`vite.config.ts`、`index.html` 等）和那一个启动入口，
其余按用途进 `docs/`、`scripts/`、`tools/`。运行期产物 `.marionette/`、`dist/`、`dist-portable/`、`node_modules/` 都已忽略。

便携构建：`tools\build-portable.bat` 默认一次打出普通版和内置引擎版。发布：`tools\publish-release.bat`。

---

## License

MIT，全文见 [LICENSE](LICENSE)。
