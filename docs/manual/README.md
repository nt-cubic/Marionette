# 开发手册（docs/manual/）

按域组织的参考手册：**这块是什么 / 改什么去哪找 / 哪些事绝不能做 / 搞完测什么**。
产品行为描述在 `docs/CURRENT.md`；实现细节的真相源在各模块头注与代码——手册是索引与红线，不复制两者。

- 模板：[_TEMPLATE.md](_TEMPLATE.md)（新域手册照它写）
- 任务走查：[howto/](howto/)（按任务而不是按域）

## 域索引

| 域 | 手册 | 一句话 |
|---|---|---|
| Agent 运行时 | [acp-agents.md](acp-agents.md) | ACP 会话、注册/自定义 agent、能力快照、注入、re-entry |
| 会话与标签 | [sessions-tabs.md](sessions-tabs.md) | 标签行/伪标签、撕窗、分屏、恢复、分组 |
| 浏览器 | [browser.md](browser.md) | 内置浏览器 surface、标签生命周期、拾取批注、ref lease、CDP 纪律 |
| Computer Use | [computer-use.md](computer-use.md) | helper 进程、封闭 Op 集、按键白名单 |
| 引擎网关 | [engine-gateway.md](engine-gateway.md) | 进程内引擎（magpie 替身）：36 模块、合同面、SKU |
| Token 用量 | [usage.md](usage.md) | 台账四桶、v2 分析页、回填、牌价 |
| 圆桌 | [council.md](council.md) | 多模型协作状态机、worktree 执行、Attention 门 |
| 贴边停靠 | [dock.md](dock.md) | 停靠几何、sliver、拖放路由、wry/tao 补丁 |
| 收纳盒 | [memo-box.md](memo-box.md) | memos 存储、整窗拖放、origin:"memo" 会话 |
| 视觉语言 | [ui-language.md](ui-language.md) | 设计系统入口（指南在 docs/guides/ui-language.md） |
| i18n | [i18n.md](i18n.md) | fe-locale 片段、复数占位符、语言钉死教训 |
| 存储与转录 | [storage-transcripts.md](storage-transcripts.md) | 转录分页、append 落盘、write_atomic、storage 锁、交付卡 |
| 内存与进程 | [memory-process.md](memory-process.md) | Job Object、工作集压制、WebView2 挂起、引擎懒启动 |
| 富文本 | [rich-markdown.md](rich-markdown.md) | 分流管线、math、shiki、SpecCard |
| 对话图片 | [seen-images.md](seen-images.md) | 右栏图片小节、定位、三态支持判定 |
| 构建与发布 | [build-release.md](build-release.md) | check.bat、build-portable、双 SKU、安装器、发布 |
| 排障 | [debug-runbook.md](debug-runbook.md) | 卡退/exe 无反应/完整性标签/焦点失灵的取证路径 |
| 员工模式 | [staff-chief.md](staff-chief.md) | chief 架构、通知与远程通道 |

## 与旧文档的关系

- 本目录是 AGENTS.md 日期段批注的**蒸馏目的地**；原文翻修批归档于 `docs/archive/`。
- `docs/pitfalls/`（事故专档）与 `docs/plans/`（设计稿+偏差记录）保持原样，手册链接过去。
- 新批次落地时的纪律：**更新对应域手册**，不再往 AGENTS.md 追加日期段；AGENTS.md 只保留宪法（全局红线 + 指针）。
