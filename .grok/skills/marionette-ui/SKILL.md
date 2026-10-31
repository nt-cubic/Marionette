---
name: marionette-ui
description: >
  Apply Marionette's conversation-stream visual language to any UI work.
  Use when changing layout, CSS, cards, empty states, forms, 圆桌/council,
  composer, event-card, 边角, 质感, 设计风格, 排版, restyle, or visual polish.
  Also when the user runs /marionette-ui.
---

# Marionette UI

先读仓库里的 `docs/ui-language.md`，再改任何用户可见界面。那是唯一真相。

对话流（`.event-card` / `.composer-mode-chip` / `.composer-select`）就是设计系统。
token 从 `src/styles/tokens.css` 和 `motion.css` 取。范例是圆桌：
`src/components/CouncilPage.tsx` + `src/styles/council.css`。

禁止用 `ui-ux-pro-max` 或其它通用设计库给 Marionette 另起圆角、阴影、字体或卡片语言。
收纳盒 8px 卡只属于收纳盒页。

动手前打开一屏真对话流，对照边角、3px 左边条、22px 芯片、6px 卡距。
改完走 `docs/ui-language.md` 的检查单。
