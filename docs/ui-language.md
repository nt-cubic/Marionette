# Marionette 视觉语言

2026-10-31 定稿。对话流就是设计系统。改任何用户可见界面（布局、样式、新页、空态、表单、卡片）前先读本文，再动手。

质感来自**跟 You / Reply / composer 共用一套表面**，不是另起皮肤、也不是把收纳盒的 8px 卡搬过来。圆桌走过「收纳盒卡 → 大标题深井 → 分段控件」，看起来像另一个产品；收到 You 卡（3px 通高左边条 + 底栏芯片）之后才和主界面对上。以后新页直接按这个做。

## 对照物（动手前打开看）

| 角色 | 文件 | 看什么 |
|---|---|---|
| token | `src/styles/tokens.css` | 色、`--radius`、`--radius-sm` |
| 动效 | `src/styles/motion.css` | `--dur-*` / `--ease-*`，`m-arrive` / `m-pop` |
| 卡片 | `src/styles/transcript.css` `.event-card` | 10px 垫、1px 边、3px 角、You 卡左边条 |
| 控件 | `src/styles/stream-cards.css` `.composer-mode-chip` / `.composer-select` / `.composer__field` | 22px 芯片、底栏 |
| 范例 | `src/components/CouncilPage.tsx` + `src/styles/council.css` | 会议室版式仍走对话流表面 |

`ui-ux-pro-max` 是通用设计库，**禁止**用它给 Marionette 生成第二套圆角/阴影/字体。

## Token（禁止发明）

从 `tokens.css` 取，不要写死 hex（主题切换靠这些变量）。

| 用途 | 变量 | 值 |
|---|---|---|
| 卡片圆角 | `--radius` | **3px** |
| 芯片 / 小控件 | `--radius-sm` | **2px** |
| 页面地面 | `--panel-muted` | |
| 卡片 | `--panel-raised` | |
| 悬停填 | `--panel-soft` | |
| 输入底栏 | `--composer-bg` | |
| 边 | `--border` / `--border-strong` | 1px solid |
| 强调 | `--accent` / `--accent-soft` | |
| 身份辅色 | `--orange` `--red` `--yellow` `--green` `--blue` | 只做点、名、左边条 |

卡片**无阴影**。`--shadow` 留给极少数浮层，不要铺在对话卡、席位卡、计划卡上。

动效：`--dur-fast` 120ms / `--dur-base` 180ms / `--dur-slow` 280ms；`--ease-standard` / `--ease-out`。入场用已有 `m-arrive` / `m-pop` / `activity-pulse`。只动 transform/opacity。尊重 `prefers-reduced-motion`。

## 卡片

```css
padding: 10px;
border: 1px solid var(--border);
border-radius: var(--radius);
background: var(--panel-raised);
overflow: hidden; /* 左边条才能贴圆角裁切 */
```

卡与卡之间 **6px**（`.event-list` 的 `gap`）。工具/思考卡可以略灰：`color-mix(in srgb, var(--panel-soft) 88%, transparent)`。

工具卡是 `30px | 1fr` 网格、`gap: 9px`，左列 30×30 图标（`--radius-sm` + `--accent-soft`）。You / Reply **没有**图标列。

## 身份色 = You 卡左边条

```css
position: relative;
overflow: hidden;
/* padding 仍是 10px，条子叠在左缘上，不占文档流 */
.card::before {
  content: "";
  position: absolute;
  left: 0; top: 0; bottom: 0;
  width: 3px;
  pointer-events: none;
  z-index: 1;
  background: var(--seat, var(--accent));
}
```

You 卡用模式色；席位/发言用成员色。成员色只出现在：**3px 左边条、7px 圆点、名字**。不要整卡填色、不要 inset box-shadow、不要 2px 且上下缩进的轨。

## 芯片与选择

对标 `.composer-mode-chip` / `.composer-select`：

- 高 **22px**，`padding: 0 7px`，`--radius-sm`，字 12px / 500
- 静息透明或极淡；悬停 `--panel-soft` + `--border`
- 选中：`color: var(--accent)`，边 `color-mix(in srgb, var(--accent) 30%, transparent)`，底 `color-mix(in srgb, var(--accent) 12%, transparent)`
- 一排离散芯片，**不要**连成三段分段条
- 原生 `<select>`：`appearance: none` + 自定义 chevron，背景透明，不要系统深井

`.pill-action` 的 **999px** 只给复制/刷新这类悬停才显形的铬按钮。状态徽标、主席、模式、阶段标签一律 `--radius-sm`。

步进器、筛选、次要操作也走 22px 芯片密度：标签 11px faint + `− 值 +`，不要各自套一张小卡。

## 表单 = 一张 You 卡 + composer 底栏

新页如果要「填完再提交」，不要做成设置页：

1. 地面 `--panel-muted`
2. 主输入是**一张** raised 卡：标题（13px / 650，底部分割线）+ 正文（13px / 1.45，`field-sizing: content`，`min-height` 约 64px，不要 8 行空井）
3. 卡底一栏芯片（模式、规则、次要旋钮），顶部分割线，像 composer 工具条
4. 页脚钉住：`--composer-bg`、顶边 1px `--border`、主按钮 28px / 12px / 600，主色走 `--accent`
5. 列表项（席位、任务、历史）是同一套 event-card，元信息一行（模型 + 职责 = composer-select 那行）

宽栏可以左右分间（召集页：左简报 / 右席位），两列的踢脚（11px / 18px 高 / `--text-faint`）必须齐顶。容器窄于约 780px 改单列。内容铺满中栏，**不要** `max-width` 居中。

## 字号

根 14px / 400 / 1.4。正文 13px / 1.45–1.55。芯片 12px。踢脚、时间、辅助 11px `--text-faint`。标题 13–14px / 650，空态可以 16px，**不要** 18–22px 营销标题。不要 `letter-spacing`。

## 收纳盒例外

`.memo-box` 的 8px 卡、帖子流、大圆角是收纳盒页自己的语言。其它产品面（对话、圆桌、用量、设置、贴边窄条）不要抄它。

## 新页检查单

- [ ] 地面 `--panel-muted`，卡 `--panel-raised` + `--radius` + 1px 边，无阴影
- [ ] 身份色是 3px 通高左边条
- [ ] 可点控件 22px / `--radius-sm`，选中态像 composer-mode-chip
- [ ] 卡距 6px；表单不是分段条 + 深井 + 小卡步进器
- [ ] 动效用现有 token 和 `m-arrive`；有 reduced-motion
- [ ] 文案走 i18n（`docs/i18n-guide.md`）
- [ ] 对照一屏真对话流（You 卡 + composer 芯片）看边角和对齐

## 回归

没有单独的视觉测试。改表面之后至少：相关 `npx tsx scripts/verify-*.mts`、`npx tsc --noEmit`。圆桌范例：`npx tsx scripts/verify-council.mts`。
