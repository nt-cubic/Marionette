# 会话与标签开发手册（sessions-tabs）

一句话：标签行（会话 + 伪标签）、分屏/撕窗、会话分组、uiRestore 恢复——
中栏「现在显示什么」的状态模型。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 标签行 | `src/components/SessionTabs.tsx` | 会话标签 + 伪标签（memo/council/usage/doc） |
| 伪标签状态族 | App.tsx 的 `memoOpen/memoActive`、`councilOpen/councilActive`、`usageOpen/usageActive`、`activeDocPath` | 同一语义族：Open=标签存在、Active=选中；**开一个关其它整页**；Esc/×/Ctrl+W 关 |
| 分屏 | SplitWorkspace reducer | session-only，surface 退出后原树恢复；dock 态禁用（提示解除贴边） |
| 撕窗（detached） | detached 相关 lib + merge-back | 关闭走隐藏复用（不 destroy——paint-assert 红线）；TrySuspend 成对 |
| 会话分组 | `src/lib/sessionGroups.ts`（`splitShelfChatSessions`/`withoutGhostProjects`） | 侧栏 chat+memo 分组；命令面板用合并列表 |
| 恢复 | `uiRestore` | 文档列表/Memo 标签/Browser URL 顺序/活动 tab/绑定会话 |
| 状态过滤 | `ProjectShelf.tsx` | 纯状态过滤保留项目行置灰（不删行——「看起来像数据丢了」教训） |
| Ctrl K 面板 | 命令面板 | usage/council/文档入口都在这 |

## 红线（绝不能做）

- **全量消费者必须走 `persistRailsNow`**：编辑重发/agent 交接/fork/撕窗合并/冷热淘汰
  ——rail 只装尾部时直接 `writeTranscript` 会吃掉磁盘历史头。
  见 [storage-transcripts.md](storage-transcripts.md)。
- **选中收纳盒/圆桌/用量伪标签时对话标签去 `is-active`（当前会话 id 记着）**；
  点回即切回；× 或 Esc 才关标签。
- **关闭有未保存内容的文档标签必须走 App 级确认**，不能从公共 tab X 绕过。
- **surface 进入时自动收起 ContextPanel，退出恢复**；Esc 归属纪律（BrowserPage
  embedded Esc 只在 target 属于浏览器根节点时生效；Composer/权限/文档/Memo 的 Esc
  不得被抢）。
- **Ctrl+W / Ctrl+Tab / Ctrl+1..9 在 surface 内先操作 content tabs**，surface 关闭后
  才是 session tab 语义。
- 撕窗**绝不 destroy**（tao paint assert 红线）；隐藏已 TrySuspend，够了。

## 坑

- 分屏态伪标签渲染在 titlebar 的 fill 前；页面覆盖整个中栏（`.center-workspace`
  只有一个 SplitWorkspace 时行改 `1fr`）。
- 会话删除/项目删除要级联关其 owned 浏览器标签；用户自开 tab 不按 owner 清理。
- 挂起巡检的可见性豁免只在 `document.visibilityState === "visible"` 时生效（藏后台
  到期照挂）；子代理在跑/ACP 后台命令/定时发送/待决权限卡的会话一律不自动挂起。

## 验证

```bash
npm run build && npx tsc --noEmit
npx tsx scripts/verify-ui-restore.mts
npx tsx scripts/verify-workspace-surface.mts
npx tsx scripts/verify-memo-box.mts   # 伪标签分组相关
```

## 已知差距与源流

- 蒸馏自：AGENTS.md「文档标签页/对话图片小节/内存压制批」相关段 + App.tsx 状态族实读。
