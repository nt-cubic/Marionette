/**
 * App-wide result toasts (bottom-right stack). For async outcomes that land
 * after the user has moved on — an engine that finished starting, a turn that
 * was undone. In-place confirmations (Copied) stay on their own button.
 */

export type ToastTone = "info" | "ok" | "error";

export type ToastAction = { label: string; run: () => void };

export type Toast = {
  id: number;
  text: string;
  detail?: string;
  tone: ToastTone;
  action?: ToastAction;
  /** ms on screen; errors stay longer. */
  duration: number;
};

type Listener = (toasts: Toast[]) => void;

const LIMIT = 6;
let nextId = 1;
let queue: Toast[] = [];
const listeners = new Set<Listener>();

function emit() {
  for (const listener of listeners) listener(queue);
}

export function showToast(input: {
  text: string;
  detail?: string;
  tone?: ToastTone;
  action?: ToastAction;
  duration?: number;
}): number {
  const tone = input.tone ?? "info";
  const toast: Toast = {
    id: nextId++,
    text: input.text,
    detail: input.detail,
    tone,
    action: input.action,
    duration: input.duration ?? (tone === "error" ? 6500 : 4000),
  };
  queue = [...queue, toast].slice(-LIMIT);
  emit();
  return toast.id;
}

export function dismissToast(id: number) {
  const next = queue.filter((toast) => toast.id !== id);
  if (next.length === queue.length) return;
  queue = next;
  emit();
}

export function subscribeToasts(listener: Listener): () => void {
  listeners.add(listener);
  listener(queue);
  return () => {
    listeners.delete(listener);
  };
}
