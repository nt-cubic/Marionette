/**
 * The docked strip's chrome (edge dock stage 2, docs/plan-edge-dock.md §3).
 *
 * Fixed bands above and below the app's own center column — the transcript
 * and composer between them are the unchanged workspace, re-flowed by the
 * `.dock-mode` CSS. The side rails become full-strip overlays opened from
 * here: 对话 opens the project/session shelf drawer, the info toggle opens
 * the ContextPanel overlay (usage / changed files / todos).
 */

import { memo, useMemo } from "react";
import { Archive, Info, ListTree, Plus } from "lucide-react";
import type { EngineQuotaEntry } from "../lib/api";

type Props = {
  onNewChat: () => void;
  onOpenShelf: () => void;
  onOpenContext: () => void;
  shelfOpen: boolean;
  contextOpen: boolean;
  /** Subscription families from `magpie quota`; worst-first summary chip. */
  quotaRows: EngineQuotaEntry[];
  onQuotaRefresh: () => void;
  onUndock: () => void;
  onQuit: () => void;
  updateAvailable: boolean;
  updateLabel: string | null;
  updateBusy: boolean;
  onCheckUpdate: () => void;
  onUpdateAndRelaunch: () => void;
  /** A split workspace is paused while docked — say so instead of squishing panes. */
  splitPaused: boolean;
};

/** One compact quota reading: `OpenCode 95%` with a hairline bar; red at the danger line. */
function QuotaChip({ rows, onRefresh }: { rows: EngineQuotaEntry[]; onRefresh: () => void }) {
  const items = useMemo(() => {
    const windows = rows.flatMap((row) =>
      row.windows
        .filter((w) => typeof w.used === "number")
        .map((w) => ({ kind: "window" as const, name: row.name, used: w.used as number })),
    );
    const balances = rows
      .filter((row) => !row.windows.length && row.balance)
      .map((row) => ({ kind: "balance" as const, name: row.name, balance: row.balance as string }));
    return [
      ...windows.sort((a, b) => b.used - a.used).slice(0, 3),
      ...balances.slice(0, 2),
    ];
  }, [rows]);

  return (
    <button
      type="button"
      className="dock-quota"
      title="模型共享订阅额度 · 最紧张的在前，点击刷新；完整分组的用量在右栏"
      onClick={onRefresh}
    >
      {items.length === 0 ? (
        <span className="dock-quota__empty">模型共享未运行</span>
      ) : (
        items.map((item) =>
          item.kind === "window" ? (
            <span className={item.used >= 90 ? "dock-quota__q is-hot" : "dock-quota__q"} key={`${item.name}-${item.used}`}>
              <span className="dock-quota__name">{item.name}</span>
              <span className="dock-quota__bar">
                <span style={{ width: `${Math.min(100, Math.round(item.used))}%` }} />
              </span>
              <b>{Math.round(item.used)}%</b>
            </span>
          ) : (
            <span className="dock-quota__q" key={`${item.name}-balance`}>
              <span className="dock-quota__name">{item.name}</span>
              <b>{item.balance}</b>
            </span>
          ),
        )
      )}
    </button>
  );
}

export const DockChrome = memo(function DockChrome({
  onNewChat,
  onOpenShelf,
  onOpenContext,
  shelfOpen,
  contextOpen,
  quotaRows,
  onQuotaRefresh,
  onUndock,
  onQuit,
  updateAvailable,
  updateLabel,
  updateBusy,
  onCheckUpdate,
  onUpdateAndRelaunch,
  splitPaused,
}: Props) {
  return (
    <>
      <div className="dock-chrome dock-chrome--top" data-tauri-drag-region="true">
        <div className="dock-chrome__actions">
          <button type="button" className="dock-chrome__btn dock-chrome__btn--primary" onClick={onNewChat} title="新建对话">
            <Plus size={13} />
            新建对话
          </button>
          <button type="button" className="dock-chrome__btn" disabled title="收纳盒 · 即将到来">
            <Archive size={13} />
            收纳盒
          </button>
          <button
            type="button"
            className={shelfOpen ? "dock-chrome__btn is-on" : "dock-chrome__btn"}
            onClick={onOpenShelf}
            aria-pressed={shelfOpen}
            title="对话列表 · 项目与全部会话"
          >
            <ListTree size={13} />
            对话
          </button>
          <button
            type="button"
            className={contextOpen ? "dock-chrome__btn is-on" : "dock-chrome__btn"}
            onClick={onOpenContext}
            aria-pressed={contextOpen}
            title="右栏 · 用量 / 改动文件 / 待办"
          >
            <Info size={13} />
          </button>
          {splitPaused && <span className="dock-chrome__hint">分屏已收起 · 解除贴边后恢复</span>}
        </div>
        <QuotaChip rows={quotaRows} onRefresh={onQuotaRefresh} />
      </div>
      <div className="dock-chrome dock-chrome--bottom" data-tauri-drag-region="true">
        <button
          type="button"
          className={updateAvailable ? "dock-chrome__txt is-update" : "dock-chrome__txt"}
          disabled={updateBusy}
          title={updateAvailable ? "下载新版本并重启" : "检查更新"}
          onClick={updateAvailable ? onUpdateAndRelaunch : onCheckUpdate}
        >
          {updateAvailable ? `升级 ${updateLabel ?? ""}`.trim() : "升级"}
        </button>
        <button type="button" className="dock-chrome__txt" onClick={onUndock} title="恢复普通窗口（尺寸与停靠前一致）">
          解除贴边
        </button>
        <button type="button" className="dock-chrome__txt dock-chrome__txt--quit" onClick={onQuit} title="停止引擎与全部会话并退出">
          退出
        </button>
      </div>
    </>
  );
});
