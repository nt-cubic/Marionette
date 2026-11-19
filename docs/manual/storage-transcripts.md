# 存储与转录开发手册（storage-transcripts）

一句话：会话转录的落盘/分页/追加、全局存储锁纪律、每轮交付卡（git 快照口径）。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 转录文件读写（tail/range 两模式） | `src-tauri/src/storage.rs`（`load_transcript_window_file` :1066） | 命令层锁内拿路径、**锁外读** |
| JSONL 追加 | `storage.rs::append_transcript_file`（:969） | 与 write_atomic 共用进程级写锁 |
| 原子写 | `storage.rs::write_atomic`（:1342） | 进程级写锁防并发 rename 互吞 + 3 轮重试 + 兜底直写 + .bak 还原 |
| 前端 rails（流式落盘） | `src/lib/useSessionRails.ts` | append 快路径 = lastPersistedRef 前缀**引用**全等 |
| 「加载更早」哨兵 | App.tsx 的 `olderHeads` + SessionView `event-list__older` | 前插后按 scrollHeight delta 平移 scrollTop |
| 交付卡（每轮改动） | `src-tauri/src/git_service.rs`（`begin_workspace_snapshot` :523 / `end_workspace_snapshot` :662）+ `src/lib/turnDelivery.ts::selectTurnWorkspaceChanges` | 归属规则见下 |

## 红线（绝不能做）

- **rail 只装尾部时直接 `writeTranscript` 会把磁盘历史头吃掉**——全量消费者
  （编辑重发 / agent 交接 / fork / 撕窗合并 / 冷热淘汰）一律走 `persistRailsNow`：
  先 `ensureFullTranscript` 补头再全量写。新写盘路径必须过它或 rails 的
  `armTranscriptTimer`。
- **持 storage 锁绝不跑子进程/不做秒级 IO**：git/交接类命令锁内只解析路径，
  子进程锁外跑（`get_changed_files` 饿死全部 storage 命令的事故实锤——69.7KB
  输出超 64KB 管道缓冲、子进程永不退出）。新命令照「锁内解析、锁外执行」形状。
- **读转录不持锁**：`load_transcript`/`load_transcript_window` 的路径版关联函数
  在锁外读；别退回 `&self` 原方法持全局锁跑顺序读。
- **append 快路径的引用判定不许克隆事件**：`persistableEventsForSession` 的
  `{...event}` 重建会让前缀引用全等永不成立 → 截断会话每次 flush 误触发静默补头。
  用 WeakMap 引用缓存。
- **交付卡 fail closed**：基线失败 / 不是 git 仓库 → 不发布任何 git 真相。
- **别用 `git status` 指纹当改动清单**（stat-dirty 假象）；口径是 worktree 内容树
  diff-tree（私有 GIT_INDEX_FILE + 私有 object dir；私有 index 必须**先复制用户
  .git/index 当 stat 缓存**，裸 read-tree 8.6s vs 0.6–1.2s）。别写回用户 index。

## 坑

- 事件 >3000 时 debounce 升 2s（450ms 全量序列化在缺页风暴下是秒级卡顿源）。
- 截断会话补头要 `publish:true` + olderHeads 走 ref+tick（异步派生，防并发更新丢失）。
- 归属规则：起点干净→结束脏无条件算；**起点就脏的必须被本轮 edit/write 工具碰过
  才算**（否则同工作区另一场对话串进本卡）。

## 验证

```bash
npx tsx scripts/verify-turn-delivery.mts     # 交付卡口径
cargo test --bin marionette git_service      # 快照/归属
cmd /c "check.bat test --bin marionette"     # storage 相关
npm run build
```

## 已知差距与源流

- 口径详述：`docs/CURRENT.md`「交付卡（每轮改动的口径）」。
- 蒸馏自：AGENTS.md「交付卡」「内存压力韧性批」0/1/2/3/6 条 + storage.rs/git_service.rs 实读。
