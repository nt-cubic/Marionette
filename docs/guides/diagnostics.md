# 诊断报告与用户日志

2026-11-18 起，用户可以在应用内生成一份可附到 GitHub Issue 的诊断报告。入口在 Ctrl K 命令面板里搜索“诊断”或“diagnostic”，选择“导出诊断报告”。

## 用户流程

1. 尽量在问题发生后、重启前导出。
2. 填写发生时间、最短复现步骤、预期结果和实际结果。
3. 保存生成的 Markdown 文件。
4. 打开 GitHub Issue，把文件拖进正文附件区。

报告由用户主动保存，不会自动上传。当前产品没有远端日志收集服务。

## 报告内容

报告是一个 Markdown 文件，包含：

- 用户填写的问题描述。
- App 版本、构建类型、系统、架构、进程号、可执行文件和日志目录。
- 当前页面、贴边状态、分屏数、窗口尺寸、WebView User-Agent、时区和主题。
- 当前会话 ID、状态、agent、模型、模式、推理档位和进程号；不包含项目名、cwd、对话正文或附件内容。
- 当前会话总数与各状态计数。
- 已缓存的 agent CLI 版本。
- 最近一次可见的 agent banner 或转录错误卡。
- 最近一次未捕获 window error 或 unhandled promise rejection（若有）。

前端全局错误与 React 根边界错误会立即写 dev.log，同时把最近一条错误摘要暂存到当前窗口的 sessionStorage；界面重载后仍能把它带进下一次诊断报告。成功导出后该暂存项会清除。
- 当前 ACP pipeline 与阻塞中的后端命令摘要。
- 最近四代 dev.log，总上限 8 MB；单个文件过大时保留文件尾。
- 最新 30 条 warn/error 的快速索引。
- 一个带 schema 版本的 JSON manifest，供后续 triage 自动化解析。

报告明确标记不包含完整对话、项目文件、凭据存储和进程环境块。

## 脱敏

前后端都不应把密钥写进日志；报告生成仍会做第二道替换，覆盖：

- Authorization / Proxy-Authorization、Cookie 与 Bearer token。
- api_key、access_token、refresh_token、password、secret、client_secret 等常见键值。
- URL query 中的 token、key、signature。
- 带用户名密码的数据库／代理 URL。
- sk-、GitHub、Slack、Google、AWS 常见密钥形态和 JWT。
- PEM 私钥块。
- 当前用户 home 路径，替换为 %USERPROFILE%。
- UI snapshot 中以敏感字段命名的值。

脱敏是防护网，不会保证用户自己写进描述或日志自由文本中的所有业务秘密都能被识别。Issue 模板要求用户上传前检查。

## 日志落盘

本机日志仍在:

~~~text
%USERPROFILE%\.marionette\logs\dev.log
%USERPROFILE%\.marionette\logs\dev.log.1
%USERPROFILE%\.marionette\logs\dev.log.2
%USERPROFILE%\.marionette\logs\dev.log.3
~~~

每代约 4 MB，info 走有界异步队列，warn/error/panic 同步落盘并 flush。诊断导出前会发送一个 best-effort flush barrier，避免刚写出的错误还停在队列里。

每行开头的方括号数字是 Unix 毫秒时间戳；manifest 的 generatedAt 使用 RFC 3339，便于自动化和日志做时间对齐。

## 机器读取约定

文件第一行是:

~~~text
<!-- marionette-diagnostic-report:v1 -->
~~~

机器读取应优先解析 “Machine-readable manifest” 下的 JSON，而不是依赖人类可读段落或日志文本。当前字段包括 schemaVersion、application、system、runtime、uiState、logSummary、redactions、included。未来版本只能增加字段或提升 schemaVersion；不得复用旧字段表达不同含义。

## 开发规则

- 新增诊断字段时优先进入 UI snapshot 或 JSON manifest，避免从自由日志猜状态。
- 不要把完整 prompt、assistant message、源文件、附件内容、项目根路径或环境变量复制进报告。
- 新增日志源若可能包含凭据，先加脱敏规则和回归测试。
- 修改报告 schema 时同步更新本文件、Issue 模板和 diagnostics.rs 的测试。

回归:

~~~powershell
cd src-tauri
cargo test --bin marionette diagnostics
~~~
