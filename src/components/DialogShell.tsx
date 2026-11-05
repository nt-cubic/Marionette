import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

/** Matches `--dur-fast` in motion.css; the exit fade must finish before onClose runs. */
const DIALOG_EXIT_MS = 120;

const DialogCloseContext = createContext<(() => void) | null>(null);

/**
 * Lets a child of DialogShell close the dialog through the same exit path as
 * Esc and backdrop clicks. Returns null outside a shell; callers fall back to
 * their own close handler in that case.
 */
export function useDialogRequestClose(): (() => void) | null {
  return useContext(DialogCloseContext);
}

function prefersReducedMotion(): boolean {
  try {
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

type DialogShellProps = {
  children: ReactNode;
  /** Called once the exit path finishes (Esc, backdrop click, or requestClose). Callers may guard this for busy states. */
  onClose: () => void;
  /** Classes for the fixed backdrop wrapper (previously each dialog's outer div). */
  className?: string;
  /** Classes for the dialog panel (previously each dialog's inner div). */
  dialogClassName?: string;
  /** Accessible name when no visible title id is available. */
  label?: string;
  /** id of the element that labels the dialog. Takes precedence over `label`. */
  labelledBy?: string;
  /** id of the element that describes the dialog. */
  describedBy?: string;
  /** ConfirmDialog uses alertdialog; everything else uses dialog. */
  role?: "dialog" | "alertdialog";
};

/**
 * Shared modal shell: backdrop, Esc, aria wiring, initial focus and the exit
 * fade. Keeps each caller's existing class names so no styling changes are needed.
 *
 * Exit: Esc / backdrop / requestClose set data-state="closing" on the backdrop
 * and panel, wait DIALOG_EXIT_MS, then call onClose. If onClose is refused by a
 * caller guard and the shell is still mounted, the closing state is cleared so
 * the dialog comes back instead of staying invisible.
 */
export function DialogShell({
  children,
  onClose,
  className,
  dialogClassName,
  label,
  labelledBy,
  describedBy,
  role = "dialog",
}: DialogShellProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const onCloseRef = useRef(onClose);
  const mountedRef = useRef(false);
  const closingRef = useRef(false);
  const timerRef = useRef<number | null>(null);
  const [closing, setClosing] = useState(false);

  useEffect(() => {
    onCloseRef.current = onClose;
  });

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      if (timerRef.current !== null) window.clearTimeout(timerRef.current);
    };
  }, []);

  const requestClose = useCallback(() => {
    if (closingRef.current) return;
    closingRef.current = true;
    if (prefersReducedMotion()) {
      onCloseRef.current();
      return;
    }
    setClosing(true);
    timerRef.current = window.setTimeout(() => {
      timerRef.current = null;
      onCloseRef.current();
      if (mountedRef.current) {
        // Still mounted: the caller refused the close (busy guard). Undo the fade.
        closingRef.current = false;
        setClosing(false);
      }
    }, DIALOG_EXIT_MS);
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        requestClose();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [requestClose]);

  useEffect(() => {
    const dialog = dialogRef.current;
    // Respect focus that children already claimed (autoFocus / ConfirmDialog cancel).
    if (dialog && !dialog.contains(document.activeElement)) {
      dialog.focus();
    }
  }, []);

  const state = closing ? "closing" : undefined;

  return (
    <div
      className={className}
      data-state={state}
      onClick={(event) => {
        if (event.target === event.currentTarget) requestClose();
      }}
    >
      <div
        ref={dialogRef}
        className={dialogClassName}
        data-state={state}
        role={role}
        aria-modal="true"
        aria-label={labelledBy ? undefined : label}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
      >
        <DialogCloseContext.Provider value={requestClose}>{children}</DialogCloseContext.Provider>
      </div>
    </div>
  );
}
