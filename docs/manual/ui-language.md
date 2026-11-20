# 视觉语言手册（ui-language）

一句话：Marionette 的设计系统入口。**规范正文在 `docs/guides/ui-language.md`**——
改任何用户可见界面前先读它；本手册只记索引与代码锚点。

## 核心事实

- **对话流（You / Reply / composer）就是设计系统**：`--radius` 3px、1px 边、无阴影、
  You 卡 3px 通高左边条、芯片 22px `--radius-sm`、卡距 6px。
- **收纳盒 8px 卡只属于收纳盒页**——别把 8px 圆角带到别处。
- **不要用通用设计库另起一套**。
- 技能：`.grok/skills/marionette-ui`（`/marionette-ui`）。
- z-index 全表：`src/styles/base-chrome.css` 头部（dock 菜单 10040、对话框
  9950–9970 一段的顺序记在那）。

## 通用纪律（跨域）

- 图表/可视化：身份靠文字、单强调色；**不要把四桶画成彩虹左边条，不要环形图冒充
  分类编码**（usage 手册同理）。
- 两色语言：`--u-accent`=缓存读、`--u-ink*`=新算（usage 图表）。
- 动效零依赖，只 transition transform/opacity，reduced-motion 整段关。
- 宽栏 780/760/620 以下断点堆叠——**量的是实际容器**（`.uv2` 是 container query；
  中栏在双侧栏+1400 宽时只有 ~830px，别按整窗调）。

## 已知差距与源流

- 规范：`docs/guides/ui-language.md`（唯一真相源）。
- 蒸馏自：AGENTS.md「视觉语言（2026-10-31）」段 + 各 styles 文件。
