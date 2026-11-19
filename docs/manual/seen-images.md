# 对话图片小节手册（seen-images）

一句话：右栏「图片」（对话里读到的图）+ 大图查看器——数据现算不新增存储。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 收集/去重/分组 | `src/lib/seenImages.ts::collectSeenImages` | You 卡 attachments + tool_call 的 images/路径；同图只算一次、定位取最近一次 |
| 节流 | `useSeenImages` | 1.5s 前沿+后沿，结果没变不换引用 |
| 右栏小节 | `src/components/seen/SeenImagesSection.tsx` | 默认一行 4 张+「+N」；展开 3 列按时间分组；可切 全部·你的·工具；一次最多 60 张；开合存 `marionette-seen-images-open` |
| 缩略图 | `SeenThumb` | 左缘 2px 来源色条：绿=你附的、`--agent-ink`=工具产生的 |
| 大图查看器 | `SeenImageViewer.tsx` | ←→ 翻页、L 定位、Esc 关；缩放拖拽 `src/lib/usePanZoom.ts`（ImageLightbox 同 hook） |
| 图片加载 | `SeenImageImg` | asset 协议懒加载，失败回退 Rust 读取 |
| 「在对话中定位」 | App 派发 `marionette-locate-seen` + SessionView `findSeenEventIndex` | 命中单元加 `is-locate-hit` 1.4s |
| 样式/i18n | `src/styles/seen-images/`；`fe-seen{shell,rail,view}` | |

## 红线（绝不能做）

- **别在 App 里直接 useMemo 扫事件**——流式每个分块都会 bump events，必须走
  `useSeenImages` 节流。
- **「在对话中定位」的下标必须按 `visibleEvents`（去掉 file_change）算**，renderUnit
  的 index 就是它的下标——别换成 displayEvents。图在折叠过程卡里时闪整张过程卡。
- **只统计已加载的对话**（`isHeadTruncated` → 页脚提示）；文案口径是「读到/附上的图」，
  **不是「模型看过」**——我们只知道发了什么。
- **能不能看图 = 三态，只信别人报的**（`src/lib/imageSupport.ts`：yes/no/unknown；
  只有明确 `no` 才变灰写「没送到它那里」，`unknown` 什么都不提示）。来源：
  agent 层 initialize 的 `promptCapabilities.image`→`agent_prompt_image: Option<bool>`
  （None=没说）；模型层 `availableModels[]` 的尽力而为解析（**没有任何按模型名猜的表**）。
- **坑**：magpie 对无元数据条目（`context_window==0`）把 `images` 默认 false——那是
  「不知道」不是「不能」；`image_flags_from_rows` 只报 `images==true` 或带元数据条目。
- **旧字段 `prompt_image: bool` 仍恒为 true（发送路径不变），别当 agent 真实声明读**。
- **别把整库解成 data URL 放进内存**——图片走 asset 协议懒加载。

## 验证

```bash
npx tsx scripts/verify-seen-images.mts   # 收集/去重/分组/定位/三态
cargo test --bin marionette -- model_entry_images model_images_come_from image_flags_skip
npm run build
```

## 已知差距与源流

- 蒸馏自：AGENTS.md「对话图片小节（2026-11-09）」段 + seenImages.ts/imageSupport.ts 实读。
