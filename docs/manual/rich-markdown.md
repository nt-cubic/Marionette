# 富文本渲染管线手册（rich-markdown）

一句话：MarkdownBody 之上的懒加载富文本——公式/代码高亮/流程图/生成式卡片，
纯字符串扫描分流 + 按需懒载。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 分流扫描 | `src/lib/richMarkdown.ts::scanRichFeatures` | 围栏状态机与 micromark 同口径（info string 按**首 token** 小写比对） |
| 公式懒载 | `src/lib/mathPack.ts::useMathPack` | remark-math + rehype-katex；`singleDollarTextMath:false` 单 `$` 恒字面 |
| math 归一化 | `normalizeMathDelimiters` | **只在围栏外**做 `\[..\]`→`$$..$$` + 整行单对 `$$..$$` 转多行 block；先 `prepareMarkdownForRender` 再归一化 |
| 代码块路由 | `MdPre` 按 code 子元素 className | `mermaid`→MdMermaid、`spec`→SpecCard、其余→MdCodeBlock |
| 高亮 | `MdShikiCode.tsx` | 精选 36 门语法表 + `createJavaScriptRegexEngine`（oniguruma 省掉）；200ms 防抖 + 120 块缓存 |
| **加语言 = 改 `LANG_LOADERS` 表** | 同上 | grammar 自带别名免费注册；额外别名进 `LANG_ALIASES`；表外回落纯文本 |
| 生成式卡片 | `SpecCard.tsx` + `specSchema.ts` | card/table/steps/chart 零依赖手绘；系列色 `.md-spec__series-N` |
| spec actions | 只派发 `SPEC_ACTION_EVENT`（`marionette:spec-action`） | App 监听后 `prefillComposer`（**只预填不自动发**） |
| 样式 | `src/styles/rich-markdown.css`（经 app.css @import，在 transcript.css 之后） | shiki 双主题 `--shiki-dark`，`[data-theme="white"]` 切 light |
| 文档页内导航 | `DocNavContext` | 只在文档页提供；默认 null=转录行为不变；**别在 MdLink 写死调用方** |

## 红线（绝不能做）

- **hooks 顺序红线**：MarkdownBody 的 hooks（scan/useMemo/useMathPack）必须在
  `if (!text) return null` **之前**——text 空/非空翻转时早退会打断 hook 链。
- **math 归一化别动围栏内**。
- **mermaid 不裁**：v12 core 无官方按需入口，深路径 DIY 对升级太脆——4.6MB 全家桶
  保留（UPX 后 ~1MB）。
- spec 解析三道防护（JSON 容错/原型污染+深度/形状+尺寸上限）+ React 错误边界；
  流式未完 JSON 安静显示「等待数据完整」+源码。
- `requireStr` 必须是 `{ok:true|false}` 判别联合（可选属性联合在真值检查下不收窄）；
  remarkPlugins 带配置的插件要显式元组类型。

## 验证

```bash
npx tsx scripts/verify-rich-markdown.mts   # scan/归一化/spec schema，24 项
npm run build
```

## 已知差距与源流

- 蒸馏自：AGENTS.md「富文本渲染管线（2026-10-26 接线定稿）」段 + 组件实读。
- 库件在 `5adf569` 入库；spec 卡片解析防护是安全边界，改前读 specSchema.ts 头注。
