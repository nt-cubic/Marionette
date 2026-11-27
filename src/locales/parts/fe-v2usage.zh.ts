/**
 * UI v2 usage copy (T3-style hierarchy: period bar, Cost/Tokens/Limits tabs,
 * first-screen summary, expandable detailed analysis).
 *
 * Values use \u escapes so the file stays ASCII in tools that cannot round-trip
 * CJK; the rendered strings are ordinary Chinese.
 */
export const zh = {
  "usageV2.view.label": "\u67e5\u770b\u65b9\u5f0f",
  "usageV2.view.cost": "\u8d39\u7528",
  "usageV2.view.tokens": "Token",
  "usageV2.view.limits": "\u989d\u5ea6",

  "usageV2.status.period": "\u6700\u8fd1 {days} \u5929",
  "usageV2.status.coverage": "\u724c\u4ef7\u8986\u76d6 {pct}",
  "usageV2.status.coverageNone": "\u724c\u4ef7\u76ee\u5f55\u4e0d\u53ef\u7528",
  "usageV2.status.limit": "\u6700\u7d27\u989d\u5ea6 {name} {pct}",
  "usageV2.status.limitNone": "\u6ca1\u6709\u53ef\u7528\u7684\u8ba2\u9605\u989d\u5ea6",
  "usageV2.status.stale": "\u4e0a\u6b21\u5feb\u7167",

  "usageV2.hero.session": "\u4f1a\u8bdd",
  "usageV2.hero.turnsCell": "\u8f6e\u6b21",
  "usageV2.hero.activeDays": "\u6d3b\u8dc3\u5929\u6570",
  "usageV2.hero.tokensCell": "Token",
  "usageV2.hero.costCell": "\u8d39\u7528",

  "usageV2.shares.title": "\u8c01\u7528\u5f97\u6700\u591a",
  "usageV2.shares.meta": "\u6309 token \u8ba1\u7b97\uff1b\u91d1\u989d\u4e3a API \u724c\u4ef7\u6298\u7b97",
  "usageV2.shares.agents": "\u6309 agent",
  "usageV2.shares.models": "\u6309\u6a21\u578b",
  "usageV2.shares.none": "\u8fd9\u6bb5\u65f6\u95f4\u6ca1\u6709\u53ef\u62c6\u5206\u7684\u7528\u91cf",
  "usageV2.shares.top": "\u6700\u9ad8 {name} {pct}",

  "usageV2.more.expand": "\u5c55\u5f00\u8be6\u7ec6\u5206\u6790",
  "usageV2.more.collapse": "\u6536\u8d77\u8be6\u7ec6\u5206\u6790",
  "usageV2.more.meta": "\u65f6\u95f4\u53bb\u5411\u3001\u5de5\u5177\u4e0e\u6587\u4ef6\u3001\u4f5c\u606f\u3001\u4f1a\u8bdd\u3001\u9010\u8f6e\u660e\u7ec6\u3001\u5386\u53f2\u56de\u586b\u4e0e CSV",

  "usageV2.limits.coverage.title": "\u724c\u4ef7\u8986\u76d6",
  "usageV2.limits.coverage.meta": "\u6309\u516c\u5f00 API \u724c\u4ef7\u6298\u7b97\uff0c\u4e0d\u662f\u8ba2\u9605\u5b9e\u4ed8",
  "usageV2.limits.coverage.rate": "\u8986\u76d6\u7387",
  "usageV2.limits.coverage.cost": "\u6298\u7b97\u91d1\u989d",
  "usageV2.limits.coverage.reported": "Agent \u81ea\u62a5",
  "usageV2.limits.coverage.peak": "\u9ad8\u5cf0\u52a0\u4ef7",
  "usageV2.limits.coverage.unavailable": "\u724c\u4ef7\u76ee\u5f55\u91cc\u6ca1\u6709\u8fd9\u4e9b\u6a21\u578b",
  "usageV2.limits.coverage.note": "\u672a\u67e5\u5230\u724c\u4ef7\u7684 token \u4e0d\u8ba1\u5165\u91d1\u989d\uff1b\u8986\u76d6\u7387 = \u5df2\u8ba1\u4ef7 token / \u5168\u90e8 token\u3002",

  "usageV2.limits.quota.title": "\u8ba2\u9605\u989d\u5ea6",
  "usageV2.limits.quota.meta": "\u6765\u81ea\u6a21\u578b\u5171\u4eab\u5f15\u64ce\uff1b\u6ca1\u6709\u6253\u5f00\u5bf9\u5e94 agent \u65f6\u53ef\u80fd\u53ea\u6709\u4e0a\u6b21\u5feb\u7167",
  "usageV2.limits.quota.empty": "\u6ca1\u6709\u53ef\u7528\u7684\u8ba2\u9605\u989d\u5ea6\u3002\u6253\u5f00\u5bf9\u5e94 agent \u7684\u5bf9\u8bdd\uff0c\u53f3\u680f\u4f1a\u663e\u793a\u5b83\u7684\u989d\u5ea6\u3002",
  "usageV2.limits.quota.balance": "\u4f59\u989d {balance}",
  "usageV2.limits.quota.resets": "{when} \u91cd\u7f6e",
  "usageV2.limits.quota.used": "\u5df2\u7528 {pct}",
  "usageV2.limits.quota.stale": "\u4e0a\u6b21\u5feb\u7167 {when}",
  "usageV2.limits.quota.staleNoWhen": "\u4e0a\u6b21\u5feb\u7167",
  "usageV2.status.customPrices": "\u542b\u81ea\u5b9a\u4e49\u4ef7\uff08\u4f30\u7b97\uff09\uff1bAgent \u81ea\u62a5\u8d39\u7528\u4f18\u5148",

  "usageV2.source.custom": "\u81ea\u5b9a\u4e49\u4ef7\uff08\u4f30\u7b97\uff09",

  "usageV2.ov.title": "\u81ea\u5b9a\u4e49\u724c\u4ef7\u4e0e\u6a21\u578b\u6620\u5c04",
  "usageV2.ov.meta": "\u81ea\u5b9a\u4e49\u4ef7\u53ea\u662f\u6298\u7b97\u4f30\u7b97\uff1bAgent \u81ea\u62a5\u8d39\u7528\u4f18\u5148",
  "usageV2.ov.reset": "\u6e05\u9664\u5168\u90e8",
  "usageV2.ov.note": "\u6bcf\u767e\u4e07 token \u7684\u4ef7\u683c\uff08USD\uff09\u3002\u53ea\u5f71\u54cd\u672a\u88ab Agent \u81ea\u62a5\u7684\u7528\u91cf\uff0c\u81ea\u62a5\u8d39\u7528\u4e0d\u4f1a\u88ab\u8986\u76d6\u3002",
  "usageV2.ov.empty": "\u8fd9\u6bb5\u65f6\u95f4\u6ca1\u6709\u6a21\u578b\u53ef\u8c03\uff0c\u4e5f\u6ca1\u6709\u5df2\u4fdd\u5b58\u7684\u8986\u76d6\u9879\u3002",
  "usageV2.ov.col.model": "\u6a21\u578b",
  "usageV2.ov.col.alias": "\u6620\u5c04\u5230",
  "usageV2.ov.col.input": "\u8f93\u5165",
  "usageV2.ov.col.output": "\u8f93\u51fa",
  "usageV2.ov.col.cacheRead": "\u7f13\u5b58\u8bfb",
  "usageV2.ov.col.cacheWrite": "\u7f13\u5b58\u5199",
  "usageV2.ov.col.filter": "\u7b5b\u9009",
  "usageV2.ov.aliasPlaceholder": "\u5982 vendor/model",
  "usageV2.ov.aliasAria": "\u628a {model} \u6620\u5c04\u5230\u2026\u2026",
  "usageV2.ov.filterAria": "\u6a21\u578b {model} \u7684\u663e\u793a\u7b5b\u9009",
  "usageV2.ov.filter.all": "\u5168\u90e8",
  "usageV2.ov.filter.include": "\u53ea\u770b",
  "usageV2.ov.filter.exclude": "\u6392\u9664",
  "usageV2.ov.filterNote": "\u7b5b\u9009\u53ea\u5f71\u54cd\u6a21\u578b\u660e\u7ec6\u4e0e\u5360\u6bd4\uff1b\u5468\u671f\u603b\u91cf\u4ecd\u5305\u542b\u5168\u90e8\u6a21\u578b\u3002",

  "usageV2.limits.coverage.customNote": "\u542b\u81ea\u5b9a\u4e49\u724c\u4ef7\u7684\u91d1\u989d\u662f\u4f30\u7b97\uff1bAgent \u81ea\u62a5\u8d39\u7528\u4f18\u5148\uff0c\u81ea\u5b9a\u4e49\u4ef7\u4e0d\u4f1a\u8986\u76d6\u5b83\u3002",

  "usageV2.schedule.action": "\u91cd\u7f6e\u540e\u53d1\u9001",
  "usageV2.schedule.prompt": "\u5230\u70b9\u53d1\u9001\u7684\u5185\u5bb9",
  "usageV2.schedule.placeholder": "\u4f8b\u5982\uff1a\u7ee7\u7eed\u521a\u624d\u7684\u4efb\u52a1",
  "usageV2.schedule.at": "\u5c06\u5728 {when} \u52a0\u5165\u5df2\u6709\u5b9a\u65f6\u53d1\u9001",
  "usageV2.schedule.confirm": "\u52a0\u5165\u5b9a\u65f6\u53d1\u9001",
  "usageV2.schedule.hint": "\u8d70\u73b0\u6709\u5b9a\u65f6\u53d1\u9001\u961f\u5217\uff0c\u53ef\u5728\u5bf9\u8bdd\u4e0a\u65b9\u53d6\u6d88\uff1b\u4e0d\u4f1a\u81ea\u52a8\u91cd\u8bd5\u3002",
  "usageV2.schedule.scheduled": "\u5df2\u52a0\u5165 {when} \u7684\u5b9a\u65f6\u53d1\u9001",
  "usageV2.schedule.saved": "\u5df2\u4fdd\u5b58\u91cd\u7f6e\u540e\u53d1\u9001\u8bf7\u6c42\uff08{when}\uff09\uff1b\u4e3b\u7a97\u53e3\u63a5\u5165\u6392\u7a0b\u540e\u624d\u4f1a\u5165\u961f",
  "usageV2.schedule.saveFailed": "\u65e0\u6cd5\u4fdd\u5b58\u91cd\u7f6e\u540e\u53d1\u9001\u8bf7\u6c42",
} as const;
