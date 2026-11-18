# Token 用量台账与分析手册（usage）

一句话：跨会话 token 统计三层链（digest→ledger→insights）+ v2 分析页 + 历史回填 +
牌价折算；引擎侧另有 usage_ledger.rs 捕获层（见 [engine-gateway.md](engine-gateway.md)）。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 记账（唯一入口） | App.tsx turn/complete 的 `recordTurnUsage` | 与 turnStats 盖戳共用单窗口不变量 |
| 本轮事件折叠 | `src/lib/usageDigest.ts` | 时间去向/工具计数/改动行数/输入估算——只扫本轮 |
| 台账存储 | `src/lib/usageLedger.ts` + `src-tauri/src/usage_store.rs` | 磁盘 `~/.marionette/usage/`：ledger.json 乐观 rev + sessions/<id>.jsonl |
| 存储后端三态 | `src/lib/usageStore.ts` | 桌面磁盘 / 浏览器 localStorage / Node 脚本内存 |
| 页面聚合（唯一数据源） | `src/lib/usageInsights.ts` | **别在组件里读台账** |
| v2 页面 | `src/components/usage/`（UsageV2Body + 分区） | 共用件只在 primitives.tsx |
| 样式 | `src/styles/usage-panel.css`、`src/styles/usage-v2/`（base.css 调色 `--uv2-*`） | `.uv2` 是 container，断点按 @container |
| 图表 | UsageCharts.tsx（TrendChart/RingMeter/RankedBars/HourHeatmap） | 手绘零依赖；两色 `--u-accent`(缓存)/`--u-ink*`(新算) |
| 牌价 | `src/lib/tokenPrices.ts` | models.dev；CSP 只放行该单域 |
| 历史回填 | `src-tauri/src/usage_backfill/`（六家 reader）+ `src/lib/usageBackfill.ts` | 契约 serde camelCase 在 mod.rs 头注释，**前端镜像两侧同步改** |
| 组成拆分（grok） | `usage_backfill/composition.rs` | 三件套→九类；实测锚 = signals.json 的 contextTokensUsed |
| 引擎侧捕获 | `engine_gateway/usage_ledger.rs` | TapReader 三方言桶；millis 请求入口起算 |

## 红线（绝不能做）

- **记账只有一处**（App.tsx recordTurnUsage）；**别为聚合重载转录**（内存压力红线）——
  新指标照 digest 折→ledger 累加→insights 汇总的链加。
- 保留 180 天、写入时修剪；多窗口读-合并-写 + storage 事件刷新，**别上锁**。
- **日期 key 一律本地钟**（dayKeyOf），别用 `toISOString`。
- **口径必须带来源**：measured/estimated/derived/priced；估算按实测总量封顶；
  agent 不报的桶就是 0/null 不编数；「等你确认」时长与压缩次数**没有数据源就别显示**。
- **百分位来自直方图**：TTFT_EDGES_MS/TPS_EDGES 桶边界只能追加不能改（旧数据失真）。
- **上下文占用只认本轮实时读数**：冻结种子值（部分 agent 只报一次的那类）不算实测。
- **别把会话累计塞进短窗口**：按「每条用量的发生日」归因（dayKeys + sessions[id]）。
- **费用三段**：reportedUsd > listed > peakExtra（仅 DeepSeek ×2，工作日时段不含
  节假日）；Grok costUsdTicks 是**会话累计，记相邻轮差值**；costOfModelUsage 是唯一
  算钱入口；文案必须带「API 牌价折算，非订阅实付」。
- **回填红线**：绝不写 agent 自己的目录、SQLite 只读、大文件流式逐行、不持 storage 锁、
  测试用根目录 override 绝不碰真实 `~/.marionette`；去重靠 `nativeSessionIds`。
- **写失败必须记 dev.log（source=usage）并重试**——「失败一次就静默关记录到重启」
  是事故，别改回去。
- i18n 占位符必须 `{count}`（t() 按 count 选复数）。

## 坑

- 组成拆分：chat_history 首行 system 与 system_prompt.txt 同一份——文件存在时跳过
  历史里的 system 行（双计实锤）；回填缓存指纹必须把三件套也计入。
- usage-smoke 在非中文系统假红——脚本开头 `await applyLocale("zh-CN")` 钉住；
  别的脚本要断言文案时照做。
- 分析页主热图是小时×星期；GitHub 式日格子（activityHeatmap.ts）留着但**不挂分析页**。
- 新增外联域先过 tauri.conf.json 的 CSP connect-src。

## 验证

```bash
npx tsx scripts/verify-usage-ledger.mts
npx tsx scripts/verify-usage-insights.mts
npx tsx scripts/usage-smoke.mts        # 含 cacheCreation 断言
npm run build
```

## 已知差距与源流

- 蒸馏自：AGENTS.md「Token 用量台账与分析面板 / v2 / v2 修正批 / 对话图片小节（无源
  字段原则）」+ usage_backfill/mod.rs 契约 + 各 lib 文件头注。
