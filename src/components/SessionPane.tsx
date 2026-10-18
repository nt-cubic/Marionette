import { memo, type ReactNode } from "react";
import type { PaneLeaf } from "../lib/splitWorkspace";

export const SessionPane = memo(function SessionPane({
  leaf,
  focused,
  onFocus,
  tabs,
  children,
}: {
  leaf: PaneLeaf;
  focused: boolean;
  onFocus: (paneId: string) => void;
  tabs: ReactNode;
  children: ReactNode;
}) {
  return (
    <div
      className={focused ? "session-pane is-focused" : "session-pane"}
      data-pane-id={leaf.id}
      onPointerDownCapture={() => {
        if (!focused) onFocus(leaf.id);
      }}
    >
      <div className="session-pane__tabs">{tabs}</div>
      <div className="session-pane__body">{children}</div>
    </div>
  );
});
