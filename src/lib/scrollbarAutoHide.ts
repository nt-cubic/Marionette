/**
 * Global scrollbar auto-hide.
 *
 * Attaches `.is-scrolling` class management to every element matching a
 * CSS selector.  The thumb is transparent by default; while scrolling
 * (and for a short idle timeout after) the class stays on so CSS can
 * fade it in.
 *
 * Usage — call once at app mount:
 *   useEffect(() => initScrollbarAutoHide(), []);
 *
 * Requires `.custom-scrollbar.scrollbar-autohide` CSS rules in app.css.
 */

const IDLE_MS = 600;

export function initScrollbarAutoHide(
  selector = ".custom-scrollbar.scrollbar-autohide",
): () => void {
  const timers = new Map<Element, ReturnType<typeof setTimeout>>();

  const onScroll = (e: Event) => {
    const el = e.currentTarget as Element;
    el.classList.add("is-scrolling");
    const existing = timers.get(el);
    if (existing) clearTimeout(existing);
    timers.set(
      el,
      setTimeout(() => {
        el.classList.remove("is-scrolling");
        timers.delete(el);
      }, IDLE_MS),
    );
  };

  const wire = (el: Element) => {
    if (el.hasAttribute("data-scrollbar-wired")) return;
    el.setAttribute("data-scrollbar-wired", "");
    el.addEventListener("scroll", onScroll, { passive: true });
  };

  // Initial scan
  document.querySelectorAll(selector).forEach(wire);

  // Watch for dynamically added scrollable containers
  const observer = new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node instanceof Element) {
          if (node.matches(selector)) wire(node);
          node.querySelectorAll(selector).forEach(wire);
        }
      }
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });

  return () => {
    observer.disconnect();
    document.querySelectorAll(selector).forEach((el) => {
      el.removeEventListener("scroll", onScroll);
    });
    for (const t of timers.values()) clearTimeout(t);
    timers.clear();
  };
}
