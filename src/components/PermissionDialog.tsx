import { ShieldAlert } from "lucide-react";
import { permissionOptionTone } from "../lib/permissionPlan";

export type PermissionOption = {
  optionId: string;
  name: string;
  kind: string;
};

export type PermissionPrompt = {
  requestId: string;
  sessionId: string;
  title: string;
  detail?: string | null;
  /** Unwrapped ExitPlanMode markdown (Claude Code "Approve Plan"). */
  planMarkdown?: string | null;
  options: PermissionOption[];
};

type PermissionDialogProps = {
  prompt: PermissionPrompt;
  busy?: boolean;
  onChoose: (optionId: string) => void;
};

/**
 * Button order: refusals on the left, then standing grants, and the one-time
 * allow last (rightmost) as the primary choice. The agent's own order breaks ties.
 */
function optionRank(opt: PermissionOption): number {
  const tone = permissionOptionTone(opt.kind, opt.name);
  if (tone === "deny") return 0;
  if (tone !== "allow") return 1;
  return /always/i.test(`${opt.kind} ${opt.name}`) ? 2 : 3;
}

export function PermissionDialog({ prompt, busy = false, onChoose }: PermissionDialogProps) {
  const options = (
    prompt.options.length > 0
      ? prompt.options
      : [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "reject", name: "Deny", kind: "reject_once" },
        ]
  )
    .map((opt, index) => ({ opt, index }))
    .sort((a, b) => optionRank(a.opt) - optionRank(b.opt) || a.index - b.index)
    .map(({ opt }) => opt);
  const firstAllow = options.findIndex((opt) => optionRank(opt) >= 1);

  return (
    <div className="permission-dialog-backdrop" role="presentation">
      <div
        className="permission-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="permission-dialog-title"
        aria-describedby="permission-dialog-desc"
      >
        <div className="permission-dialog__header">
          <ShieldAlert size={18} aria-hidden />
          <div>
            <span className="permission-dialog__eyebrow">需要你批准</span>
            <strong id="permission-dialog-title">{prompt.title || "Permission required"}</strong>
          </div>
        </div>
        {prompt.detail && (
          <pre className="permission-dialog__detail" id="permission-dialog-desc">{prompt.detail}</pre>
        )}
        <div className="permission-dialog__actions">
          {options.map((opt, index) => {
            const tone = permissionOptionTone(opt.kind, opt.name);
            // The allow group starts after the refusals; that gap is the split.
            const split = index === firstAllow && firstAllow > 0;
            return (
              <button
                key={opt.optionId}
                type="button"
                className={`permission-dialog__btn permission-dialog__btn--${tone}${split ? " is-split" : ""}`}
                disabled={busy}
                onClick={() => onChoose(opt.optionId)}
              >
                {opt.name || opt.optionId}
              </button>
            );
          })}
        </div>
      </div>
    </div>
  );
}
