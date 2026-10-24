import { useCallback, useRef } from "react";

/**
 * Latest-ref callback：返回的函数在组件生命周期内引用稳定，但每次调用都
 * 落到最新一次渲染的 `fn`。给喂给 memo 化子组件（ProjectShelf /
 * SessionTabs）的事件处理器用——引用稳定让 `React.memo` 在流式
 * eventsTick（20-50Hz）重渲时保持跳过，ref 转发让闭包永远不过期。
 * 与 App.tsx 里 `openSessionRef` 的既有约定同源。
 */
export function useLatestCallback<A extends unknown[], R>(
  fn: (...args: A) => R,
): (...args: A) => R {
  const ref = useRef(fn);
  ref.current = fn;
  return useCallback((...args: A) => ref.current(...args), []);
}
