/**
 * Native drop routing for the docked strip (2026-10-26).
 *
 * An Explorer drag over the strip used to die silently unless it happened to
 * land on the composer field — everything above (chrome, 进行中) swallowed
 * it. Now the whole strip answers, with the composer's drop-hint look:
 *
 *   home view       top band → 收纳盒 · below → composer attach
 *   收纳盒 page     the memo page owns the whole strip (this router stands
 *                   down — MemoBox accepts drops anywhere in the window)
 *   other pages     whole strip → 收纳盒 (no conversation is in view)
 *   no conversation whole strip → 收纳盒 (nothing to attach to)
 *
 * The composer's own Tauri drop handler is off while docked; chat-zone paths
 * travel through lib/dockDrop.ts's DOM event. The veil is pointer-events:
 * none — it never intercepts, it only labels the two zones.
 */

import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { isTauriRuntime } from "../lib/api";
import { isPageDragSource, onDragText } from "../lib/dragText";
import { dispatchDockDropPaths, dispatchDockDropText } from "../lib/dockDrop";
import { memoList, memoSavePaths, memoSaveText } from "../lib/memos";
import { showToast } from "../lib/toast";
import type { DockPage } from "./DockChrome";

type DropZone = "memo" | "chat";

type Props = {
  page: DockPage | null;
  /** Whether a composer is mounted to receive chat-zone drops. */
  hasConversation: boolean;
  onMemoCount: (count: number) => void;
};

export function DockDropRouter({ page, hasConversation, onMemoCount }: Props) {
  const [zone, setZone] = useState<DropZone | null>(null);
  const live = useRef({ page, hasConversation, onMemoCount });
  live.current = { page, hasConversation, onMemoCount };

  /** Bottom of the memo zone on the home view: the live chrome band. (Full
   *  pages route the whole strip to 收纳盒, so only the home view splits.) */
  const splitTop = () => {
    const el = document.querySelector<HTMLElement>(".dock-chrome");
    return el ? el.getBoundingClientRect().bottom : 0;
  };

  const zoneAt = (y: number): DropZone | null => {
    const { page, hasConversation } = live.current;
    if (page || !hasConversation) return "memo";
    return y <= splitTop() ? "memo" : "chat";
  };

  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    // Armed only by an `enter` that actually carries OS paths — tao also pings
    // for in-page memo drags, which carry none and must stay pure HTML5.
    let armed = false;

    const saveMemos = (paths: string[]) => {
      void memoSavePaths(paths, "拖入")
        .then((rows) => {
          live.current.onMemoCount(rows.length);
          showToast({ text: `已存入收纳盒 · ${paths.length} 项` });
        })
        .catch((error) =>
          showToast({ text: "没能存进收纳盒", detail: String(error), tone: "error" }),
        );
    };

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (disposed) return;
        // While the 收纳盒 page is open, MemoBox's own listener owns the
        // whole window (accepts drops anywhere) — standing down avoids a
        // double save.
        if (live.current.page === "memo") {
          armed = false;
          setZone(null);
          return;
        }
        const payload = event.payload;
        if (payload.type === "leave") {
          armed = false;
          setZone(null);
          return;
        }
        const pos = "position" in payload ? payload.position : null;
        if (!pos) return;
        // Tauri reports physical pixels; scale back to CSS coords.
        const y = pos.y / (window.devicePixelRatio || 1);

        if (payload.type === "enter") {
          armed = Boolean(payload.paths?.length);
          setZone(armed ? zoneAt(y) : null);
          return;
        }
        if (!armed) return;
        if (payload.type === "over") {
          setZone(zoneAt(y));
          return;
        }
        // drop
        armed = false;
        setZone(null);
        const paths = payload.paths ?? [];
        if (!paths.length) return;
        const target = zoneAt(y);
        if (target === "chat") dispatchDockDropPaths(paths);
        else if (target === "memo") saveMemos(paths);
      })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // Text/link drags (vendor wry patch channel) ride the same zones as files:
  // memo band → 收纳盒 (Rust turns bare URLs into link entries), chat zone →
  // composer as text. The 收纳盒 page owns the whole strip, same as files.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void onDragText((event) => {
      if (disposed || isPageDragSource()) return;
      if (live.current.page === "memo") {
        setZone(null);
        return;
      }
      if (event.kind === "leave") {
        setZone(null);
        return;
      }
      if (event.kind === "enter" || event.kind === "over") {
        setZone(zoneAt(event.y / (window.devicePixelRatio || 1)));
        return;
      }
      if (event.kind !== "drop") return;
      setZone(null);
      const text = event.text.trim();
      if (!text) return;
      const target = zoneAt(event.y / (window.devicePixelRatio || 1));
      if (target === "chat") {
        dispatchDockDropText(text);
        return;
      }
      void memoSaveText(text, undefined, "拖入")
        .then((saved) => {
          if (!saved) throw new Error("save failed");
          return memoList().then((rows) => {
            live.current.onMemoCount(rows.length);
            showToast({ text: "已存入收纳盒" });
          });
        })
        .catch(() => showToast({ text: "没能存进收纳盒", tone: "error" }));
    })
      .then((dispose) => {
        if (disposed) dispose();
        else unlisten = dispose;
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  if (!zone) return null;
  // 收纳盒 page: the page's own drop UI covers the whole window.
  if (page === "memo") return null;
  // No conversation in view (other full pages, or no dialog at all).
  if (page || !hasConversation) {
    return zone === "memo" ? (
      <div className="dock-drop-veil is-memo is-full">松开 · 存入收纳盒</div>
    ) : null;
  }
  return (
    <>
      <div className={zone === "memo" ? "dock-drop-veil is-memo is-hot" : "dock-drop-veil is-memo"}>
        {zone === "memo" ? "松开 · 存入收纳盒" : "收入收纳盒"}
      </div>
      <div className={zone === "chat" ? "dock-drop-veil is-chat is-hot" : "dock-drop-veil is-chat"}>
        {zone === "chat" ? "松开 · 放入对话输入框" : "放入对话输入框"}
      </div>
    </>
  );
}
