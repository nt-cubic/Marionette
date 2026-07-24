import { ShieldAlert } from "lucide-react";

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
  options: PermissionOption[];
};

type PermissionDialogProps = {
  prompt: PermissionPrompt;
  busy?: boolean;
  onChoose: (optionId: string) => void;
};

function optionTone(kind: string, name: string): "allow" | "deny" | "neutral" {
  const k = `${kind} ${name}`.toLowerCase();
  if (k.includes("allow") || k.includes("accept") || k.includes("yes")) return "allow";
  if (k.includes("reject") || k.includes("deny") || k.includes("cancel") || k.includes("no")) {
    return "deny";
  }
  return "neutral";
}

export function PermissionDialog({ prompt, busy = false, onChoose }: PermissionDialogProps) {
  const options =
    prompt.options.length > 0
      ? prompt.options
      : [
          { optionId: "allow", name: "Allow once", kind: "allow_once" },
          { optionId: "reject", name: "Deny", kind: "reject_once" },
        ];

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
            <strong id="permission-dialog-title">Permission required</strong>
            <span id="permission-dialog-desc">{prompt.title}</span>
          </div>
        </div>
        {prompt.detail && (
          <pre className="permission-dialog__detail">{prompt.detail}</pre>
        )}
        <div className="permission-dialog__actions">
          {options.map((opt) => {
            const tone = optionTone(opt.kind, opt.name);
            return (
              <button
                key={opt.optionId}
                type="button"
                className={`permission-dialog__btn permission-dialog__btn--${tone}`}
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
