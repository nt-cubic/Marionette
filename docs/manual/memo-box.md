# 收纳盒（memo-box）开发手册

一句话：karakeep 式「收集→整理成对话」——文字/链接/图片/文件收进 `~/.marionette/memos/`，
整窗接住 OS 拖放，一键整理成对话或文档。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 存储层 | `src-tauri/src/memos.rs` → `~/.marionette/memos/` | `index.json`（原子写、读失败 fail-open 空表、500 条上限）+ `files/`（内容哈希命名，重拖同内容=同条目；文件夹整树复制不去重） |
| 页面组件 | `src/components/MemoBox.tsx` | 无 `showClose`（标题与关闭在挂载点铬里） |
| 侧栏分组 | `ProjectShelf.tsx` 的 `memoSessions` prop + `src/lib/sessionGroups.ts::splitShelfChatSessions` | 聊天下方「收纳盒」分组，非空才显示 |
| 伪标签挂载 | App.tsx `memoOpen`（标签存在）/`memoActive`（选中）；`SessionTabs` 的 `memoTab` prop | 普通窗口=标签页不是全屏覆盖；贴边=DockPage 整页 |
| 整窗拖放 | `MemoBox` 的 onDragDropEvent | **不做 rect 命中测试**（见红线） |
| 命令 | `create_chat_session` 的 `origin` 参数 | 白名单只认 `"memo"`，其余回落 `"user"` |
| 幽灵项目过滤 | `sessionGroups.ts::withoutGhostProjects` | 旧 inbox 项目（rootPath→`~/.marionette/inbox`） |

## 红线（绝不能做）

- **memo 页激活时整窗接住 OS 拖放，不做 rect 命中测试**：旧实现只认页面矩形，
  拖到侧栏/标签行=无声 no-op（本机 memos 目录从未落盘的事故根因）。`enter` 无
  paths 不武装（页内 memo 行拖拽保持纯 HTML5 链路）。
- **贴边态 `DockDropRouter` 在 page==="memo" 时整体让路**——否则路由器标题带分支
  会与页面监听**双保存**。
- **「开新对话」一律建 `origin:"memo"` 的 Chat 会话**：memo 会话在运行时**就是普通
  Chat 会话**（spawn cwd/prefs/标签全走 CHAT_PROJECT_ID 原路径）——**别给它开新
  特判分支**。「整理成文档」开着项目仍建进项目，没开项目落收纳盒组。
- **别恢复 inbox 收集箱项目**（10-22 方案遗留，已清）。
- **图片卡禁止定高 `cover` 裁切**：`memoImageOrient` 按横竖比完整显示（横图铺满宽、
  竖图触顶后收窄，`object-fit:contain`）。

## 交互清单（现状，改前对照）

1. 整窗拖入（OS 级）；页内拖到输入框（HTML5）；多选开新对话/整理成文档。
2. 单列帖子流：顶部 `memo-box__compose`（Ctrl+Enter 保存，裸 URL 走 `memo_save_text`
   自动转 link 条目）；卡片 `.memo-box__card--text/image/link/file`；按
   `groupMemosByDay` 插「今天/昨天/N月N日」。
3. 点文字卡原地展开（6 行 clamp）；链接/图片/文件走 `openExternal`。
4. 选中收纳盒时对话标签去 `is-active`（当前会话 id 记着）；×或 Esc 才关标签。
5. 贴边入口由 DockChrome 提供；dock 态上半条（chrome 带）拖放→收纳盒。

## 验证

```bash
npx tsx scripts/verify-memo-box.mts
npm run build && npx tsc --noEmit
```
真机：拖文件进 memo 页（含贴边态）→ 开新对话带材料清单 → 重拖同内容不出新条目。

## 已知差距与源流

- 蒸馏自：AGENTS.md「贴边停靠」段的收纳盒小节（存储/交互梳理/呈现三块）+
  memos.rs/MemoBox.tsx 实读。
