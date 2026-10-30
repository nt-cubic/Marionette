# Marionette i18n 转换指南（zh-CN ⇄ en）

2026-10-30 落地。前端走词典键（`src/locales/`），Rust 走 `ts!` 就地双语。
改任何用户可见文案前先读完本文；开发向文档（AGENTS.md、docs/、日志、代码注释）保持中文，不翻译。

## 前端

### 基础设施（已就绪，勿重复造）

- `src/lib/i18n.ts`：模块级 locale store + `t(key, params?)`（`{name}` 插值；`params.count`
  为数字时 en 优先查 `key_one`/`key_other` 复数变体）+ `useT()` / `useLocale()`
  （`useSyncExternalStore`，语言切换即时重渲染，memo/portal 均不受阻）。
- 词典：`src/locales/zh-CN.ts` / `en.ts` 合并 `src/locales/parts/*.{zh,en}.ts` 片段。
  zh 为源（`as const`），en 用 `satisfies Record<keyof typeof zh, string>` —— **缺英文键 tsc 直接报错**。

### 转换规则

1. **组件**（.tsx）：`const t = useT();` 一次，字符串换 `t("ns.key")`。JSX 文本、
   title/aria-label/placeholder、选项数组标签、三元分支（保持结构 `x ? t("a") : t("b")`）全换。
2. **非组件 lib**（.ts）：`import { t } from "./i18n"` 直接调模块级 `t`——它在事件期被调用，
   读到的永远是当前语言。
3. **插值**：`` `${n} 分钟前` `` → `t("common.minutesAgo", { n })`，词典值 `"{n} 分钟前"`。
   键名用 camelCase 参数。
4. **useMemo/useCallback 里算出的字符串**：依赖数组加 `locale`（`const locale = useLocale()`），
   否则切语言不重算（参见 `CommandPalette.tsx` 的 groups memo）。
5. **复数**：en 需要单复数时给 `key_one`/`key_other` 两个键（zh 只有基础键）。
6. **键名**：`域名.语义名`（`shelf.noConversations`、`errors.needLogin`）。跨面复用的短字符串
   （按钮、相对时间、今天/昨天）先查 `parts/common.*`，有就复用，没有再进自己批次的片段。
7. **片段文件**：每个转换批次拥有一对 `parts/<batch>.zh.ts` / `<batch>.en.ts`，en 侧
   `import type { zh } from "./<batch>.zh"` + `satisfies`。**并行批次绝不互相改文件**；
   `zh-CN.ts`/`en.ts`/`i18n.ts`/`common.*` 只由维护者改。
8. 参照样例（已转换，可直接读）：`src/lib/theme.ts`、`src/components/CommandPalette.tsx`、
   `App.tsx` 的命令面板 actions 数组。

### 例外（不进词典）

- `src/lib/mockData.ts`（浏览器预览假数据）、`src/locales/**`、代码注释。
- 传给模型的指令文本若包含 UI 文案字符串，照常转换（UI 展示优先）。

## Rust（`src-tauri/src/loc.rs`）

- `ts!("中文 {x}", "English {x}")` —— 按全局语言二选一 `format!`；隐式捕获 `{x}` 直接用，
  两个字面量的占位符必须一致（两个分支都会被编译器检查）。需要 &str 时 `&ts!(...)`。
- 语言来源：启动 `loc::init_from_disk()` 读 `~/.marionette/language`；前端每次启动/切换经
  `set_ui_locale` 命令推送（前端是 truth 的持有者）。
- **转**：Tauri command 的 `Result<_, String>` 错误与成功消息、原生弹窗、Telegram bot 模板、
  内存/模型/引擎目录里显示给用户的标签。
- **不转**：日志行（`debug_log::append` 的 message 保持英文/现状）、发给模型的 prompt
  （grok_dialect 等按设计就是中文）、`#[cfg(test)]` 内的断言与 fixture、代码注释。

## 术语表（en 一致性的底线）

| 中文 | English |
|---|---|
| 会话 | session |
| 对话 | conversation |
| 聊天 | chat |
| 项目 | project |
| 收纳盒 | Memo Box |
| 贴边停靠 / 贴边 | edge dock / docked |
| 窄条 | strip |
| 解除贴边 | undock |
| 模型共享 | Model Share |
| 引擎 | engine |
| 交付卡 | delivery card |
| 用量 | usage |
| 额度 | quota |
| 凭据 | credentials |
| 挂起 | park |
| 分屏 | split view |
| 标签（页） | tab |
| 收起/展开 | collapse / expand |
| 检查更新 | Check for updates |
| 长期记忆 | long-term memory |
| 能力与上下文 | Capabilities & context |
| 权限 | permission |
| 批准 | approve |
| 计划 | plan |
| 子代理 | subagent |
| 手递手 | handoff |
| Telegram 远程 | Telegram remote |
| 代理 | proxy |
| 运行中 / 等你 / 已完成 / 出错 | Running / Waiting on you / Done / Error |
| 已超时 | Timed out |
| 主题：浅夜 / 纯白 | Theme: Dusk / Pure White |
| 模型 | model |
| 推理档位 | reasoning effort |

## 验收

```bash
npx tsx scripts/inventory-zh.mts --check   # 前端：非白名单文件 CJK（注释除外）必须为 0
npx tsc --noEmit                            # en 词典缺键在此拦截
node scripts/check-locale-fragments.mjs src/locales/parts/<你的批>.zh.ts   # 片段级快检
cargo test                                  # Rust 默认 zh，旧断言原样通过
```

前端功能性例外（**不要**为了清零去转它们）：zh 疑问句/语气词检测正则（suggestions.ts、
todos.ts、linkTargets.ts 等）、`startsWith("正在重试")` 这类**双语并集**探测
（App.tsx / acpTranscript.ts / sessionHistory.ts / ModelLibraryDialog.tsx 已按
「两种语言都匹配」写好，新探测点照此办理）、legacy 持久化标签比对
（transcript.ts `shouldAutoRenameLabel`）。Rust 侧模型向文本（memory prompt、
transcript_digest、grok_dialect、`<marionette-memory>` 抬头）保持中文。
