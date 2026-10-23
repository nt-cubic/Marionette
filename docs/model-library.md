# 模型与来源

模型共享浮层中的「模型与来源 · 显示和排序」管理模型菜单。按来源和账号区分模型；只有接口、认证和协议确认对应同一来源时，直连、原生和引擎入口才合并。同名模型不作为去重依据。

## 同一账号只列一次

同一个账号常在多处出现：agent 自带的 provider（如 OpenCode 的 `deepseek`）、Grok 的 `[model.*]` 表、以及共享引擎的目录。Marionette 用**接口主机 + 凭证**判断它们是不是同一账号：

- 主机相同，并且解析出的 key 相同，或两侧引用同一个环境变量名，就并入同一个分组；分组标题采用 agent 自己对这家厂商的称呼（`DeepSeek`），接口主机名（`deepseek.com`）只在没有 agent 列出该账号时使用。
- 路径形态不参与判断：`…/inference/go/openai/v1` 与 `…/inference/go/v1` 是同一家；带会话头也不影响账号身份。
- 模型名相同但主机或凭证不同的条目保持分开——不同厂商都会发 `deepseek-flash` 这类名字。

合并后每个模型只占一行，行上保留 agent 自己的模型 id，切到该行仍走 agent 原生通道。合并前保存的隐藏与排序按旧标识迁移到合并后的行，已经隐藏的模型不会因为合并重新出现。

## 一次订阅登录只列一次

订阅型登录态（`~/.codex/auth.json` 的 ChatGPT、`~/.grok/auth.json` 的 xAI）同时经两条通道出现：引擎把它翻成 `codex/…`、`grok/…` 路由，agent CLI 自己又列一份模型名。两条通道读同一份登录文件，因此算同一个账号：

- 拥有登录文件的 agent 整份目录并入引擎分组，标题用引擎侧的称呼（`ChatGPT`、`Grok 订阅`），行上保留 agent 原生 id。
- 另一个 agent 若把同一份 OAuth 登录复制到自己的凭据里（OpenCode auth.json 的 `openai`），其 `metadata.accountId` 与 Codex 的 `tokens.account_id` 相同时视为重复，不再单列——这 24 个 `openai/*` 是 OpenAI 按量 API 的全量目录，与订阅侧条目形态不同。
- 只有 OAuth 登录参与比对；同名 provider 下的 API key 是另一份凭证，照常单独成组。
- 引擎未运行时该分组没有行，复制的登录会重新出现，作为唯一可用入口。


## 显示设置

- provider 的复选框代表组内模型全选／全不选，部分选中显示半选；至少选中一个模型时才显示该 provider。模型始终可以单独勾选。旧版隐藏 provider 设置会转换为组内模型全不选。
- 上下箭头分别调整 provider 顺序和组内模型顺序；搜索时仍按完整列表移动。箭头在指针移到该行或键盘聚焦时出现，其余时间收起来让列表读起来是模型。
- 模型超过 8 个的分组默认收起，只列出已勾选的模型，组尾「显示全部 N 个模型…」展开完整目录；点分组标题前的箭头也可开合。搜索时所有匹配始终展开。
- 行标题省略与分组标题重复的厂商前缀（`Ollama Cloud/Beam` 在 `ollama.com` 分组里显示为 `Beam`），原始模型 id 保留在第二行小字里。
- 隐藏只影响选择菜单和最近使用，不删除来源，不更改当前会话。
- 保存后应用到各 agent 的菜单和其他窗口。新发现的条目默认显示，排在手动排序的条目之后。
- 设置保存在 `~/.marionette/model-library.json`；恢复默认会恢复全部显示和默认顺序。
- 原生模型目录来自各 agent 的 ACP 广告，首次连接前尚未缓存的原生条目会在连接后出现。

## 打开方式

- 模型菜单底部「配置模型与来源…」直接打开本页；再下面的「添加 API Key…」打开凭据页。
- 左下角模型共享浮层的底部有两个入口：上面一行是「模型与来源 · 显示和排序…」，下面一行是「管理密钥与来源…」，各自占一行。

## 已有配置的发现

默认检查下列用户配置，原文件保持只读：

| 来源 | 位置与内容 |
|---|---|
| Grok Build | `~/.grok/config.toml` 中的 `[model.*]`，包括 `model`、`name`、`base_url`、`env_key` / `api_key`、`extra_headers`；尊重 `GROK_HOME` |
| Pi | `~/.pi/agent/models.json` 中的 `providers`；尊重 `PI_CODING_AGENT_DIR`；按模型保留协议和接口覆盖 |
| OpenCode | `~/.config/opencode/opencode.json` / `.jsonc` 中的自定义 `provider`；尊重 `XDG_CONFIG_HOME` / `OPENCODE_CONFIG`；认证复用已保存的 `auth.json`，包括 Windows 路径兜底 |
| Codex | `~/.codex/config.toml` 中的 `model_providers` 与当前显式模型；尊重 `CODEX_HOME` |
| Claude Code | `~/.claude/settings.json` 中的接口、认证和显式模型环境设置；尊重 `CLAUDE_CONFIG_DIR` |

凭据页的「扫描位置与结果」列出实际检查的绝对路径，分别显示文件不存在、已读取、读取或解析失败。凭据摘要兼容 Rust 的按 agent 返回格式，扫描到的引用也显示在列表里。

支持三个共享协议：OpenAI Chat Completions、OpenAI Responses、Anthropic Messages。认证支持原配置中的 key、环境变量引用、OpenCode 的文件引用。发现的配置按需同步到 Marionette 管理的引擎，其他已接桥 agent 无需再录入相同 key。重开模型菜单会刷新过期的来源缓存；「重新扫描」立即刷新。

缺失环境变量、无法解密、无法读取文件、未知协议等情况显示原因。缺失环境变量可在来源下方填写并保存到 Windows 用户环境变量，立即应用到 Marionette 并刷新来源；已运行的 agent 需重新连接。在终端临时设置的变量不会自动出现在已运行的 GUI 进程中。扫描不会执行认证命令，也不会运行配置里的 JavaScript。DSH 的动态 Cordis provider、其他未适配格式和项目级覆盖尚未归入这批用户配置扫描。

## 添加自定义来源

「API 与登录凭证」默认只展示一个凭证列表；添加表单通过「添加 API」展开。文件路径、扫描结果、警告和引擎状态统一放在默认收起的「详情与诊断」。API 的「检测」使用已解析的认证请求模型目录，超时 8 秒，不发送生成请求；目录可访问不代表余额和模型调用权限已验证。登录凭证只标记已保存登录态。

凭据页「添加 API → 服务商 → 自定义」填写来源名称、接口基础地址、协议、API key 和模型 ID。模型 ID 可每行一个或用逗号分隔；留空则由引擎尝试获取服务端目录。无需认证的服务可不填 key。

相同名称、地址和协议再次保存视为换钥匙；不同账号可用不同名称保存。手动 key 继续使用原有 DPAPI 存储；来源元数据与 key 同时删除。自定义来源通过共享引擎提供给各个已接桥 agent。

模型来源与 agent 的模式、推理档位、权限和会话偏好分开保存；显示设置不会改写这些偏好。
