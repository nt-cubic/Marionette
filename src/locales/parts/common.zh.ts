/**
 * Shared dictionary fragment: short strings reused across many surfaces
 * (buttons, empty states, relative time, day buckets). Owned by the i18n
 * bootstrap work — batch fragments under parts/ MUST reuse these keys
 * instead of re-adding the same wording under their own namespace.
 */
export const zh = {
  "common.languageToggle": "语言：{current}（点击切换到 {next}）",
  "common.chat": "聊天",
  "common.cancel": "取消",
  "common.confirm": "确定",
  "common.close": "关闭",
  "common.delete": "删除",
  "common.remove": "移除",
  "common.retry": "重试",
  "common.copy": "复制",
  "common.save": "保存",
  "common.open": "打开",
  "common.search": "搜索",
  "common.today": "今天",
  "common.yesterday": "昨天",
  "common.thisWeek": "本周",
  "common.earlier": "更早",
  "common.on": "开",
  "common.off": "关",
  "common.minutesAgo": "{n} 分钟前",
  "common.hoursAgo": "{n} 小时前",
  "common.daysAgo": "{n} 天前",
  "common.loading": "加载中…",
  "common.listSep": "、",
} as const;
