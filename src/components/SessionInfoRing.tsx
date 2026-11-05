import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
import { Check, Copy } from "lucide-react";
import { agentInkStyle } from "../lib/agentInk";
import { useT } from "../lib/i18n";
import {
  buildInfoFigures,
  contextPercent,
  formatContextCount,
  ringTone,
  type SessionInfoData,
} from "../lib/sessionInfo";

/**
 * Session-info ring: the 16px context-occupancy ring left of the send button
 * and the popover it opens (docs/ui-language.md v2 "输入框", LightC.html).
 *
 * The popover is a `.composer-menu` driven by the Composer's own `menu` state
 * (id "info"), so the Composer's viewport-clamp pass positions it. It stays
 * mounted for the 120ms exit; the clamp pass resets its inline left/right the
 * moment `menu` changes, so the last clamped offsets are captured while open
 * and re-applied for the exit frames.
 */

const RING_R = 6;
const RING_C = 2 * Math.PI * RING_R;

type Props = {
  agentId: string;
  agentLabel: string;
  /** Short model name for the header and the detail group; null when unknown. */
  modelName: string | null;
  contextUsed: number | null;
  contextSize: number | null;
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  /** Lazy so a closed popover costs nothing; null = no statistics available. */
  getInfo?: (() => SessionInfoData | null) | null;
};

export function SessionInfoRing({
  agentId,
  agentLabel,
  modelName,
  contextUsed,
  contextSize,
  open,
  onToggle,
  onClose,
  getInfo,
}: Props) {
  const t = useT();
  const anchorRef = useRef<HTMLDivElement>(null);
  const pct = contextPercent(contextUsed, contextSize);
  const tone = ringTone(pct);
  const fraction = pct == null ? 0 : pct / 100;
  const pctText = pct == null ? null : `${Math.round(pct)}%`;
  const ariaLabel =
    pctText == null
      ? t("composer.info.aria")
      : t("composer.info.ariaWithPct", { pct: pctText });

  // Outside pointer-down closes (the Composer has no global dismiss of its own).
  useEffect(() => {
    if (!open) return;
    const onDown = (event: PointerEvent) => {
      const anchor = anchorRef.current;
      if (anchor && event.target instanceof Node && anchor.contains(event.target)) return;
      onClose();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);

  return (
    <div
      ref={anchorRef}
      className="composer-menu-anchor composer-menu-anchor--info"
      style={agentInkStyle(agentId)}
    >
      <button
        type="button"
        className={`session-info-ring is-${tone}`}
        aria-label={ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={ariaLabel}
        onClick={onToggle}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true">
          <circle className="session-info-ring__track" cx="8" cy="8" r={RING_R} />
          {fraction > 0 && (
            <circle
              className="session-info-ring__fill"
              cx="8"
              cy="8"
              r={RING_R}
              strokeDasharray={`${(fraction * RING_C).toFixed(2)} ${RING_C.toFixed(2)}`}
              transform="rotate(-90 8 8)"
            />
          )}
        </svg>
      </button>
      <InfoPopover
        open={open}
        tone={tone}
        agentLabel={agentLabel}
        modelName={modelName}
        contextUsed={contextUsed}
        contextSize={contextSize}
        pct={pct}
        info={open ? (getInfo?.() ?? null) : null}
      />
    </div>
  );
}

function InfoPopover({
  open,
  tone,
  agentLabel,
  modelName,
  contextUsed,
  contextSize,
  pct,
  info,
}: {
  open: boolean;
  tone: "ok" | "warn" | "full";
  agentLabel: string;
  modelName: string | null;
  contextUsed: number | null;
  contextSize: number | null;
  pct: number | null;
  info: SessionInfoData | null;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  const [closing, setClosing] = useState(false);
  const [frozen, setFrozen] = useState<CSSProperties | null>(null);
  const [copied, setCopied] = useState(false);
  /** Last figures shown, so the exit frames keep their content. */
  const lastInfo = useRef<SessionInfoData | null>(null);
  const clamped = useRef<CSSProperties | null>(null);
  const wasOpen = useRef(false);

  if (open) lastInfo.current = info;

  // Track the clamp pass's inline offsets while open: the Composer blanks them
  // in its own cleanup the moment `menu` changes, so they must be read earlier.
  useLayoutEffect(() => {
    if (!open) return;
    const el = ref.current;
    if (!el) return;
    const read = () => {
      clamped.current = { left: el.style.left, right: el.style.right };
    };
    read();
    const mo = new MutationObserver(read);
    mo.observe(el, { attributes: true, attributeFilter: ["style"] });
    return () => {
      // Pending records fire after the cleanup that blanked the style; drop
      // them and keep the last clamped reading.
      mo.takeRecords();
      mo.disconnect();
    };
  }, [open]);

  useEffect(() => {
    if (open) {
      wasOpen.current = true;
      setClosing(false);
      setFrozen(null);
      return;
    }
    if (!wasOpen.current) return;
    wasOpen.current = false;
    setFrozen(clamped.current);
    setClosing(true);
    const timer = window.setTimeout(() => {
      setClosing(false);
      setFrozen(null);
    }, 120);
    return () => window.clearTimeout(timer);
  }, [open]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 1400);
    return () => window.clearTimeout(timer);
  }, [copied]);

  if (!open && !closing) return null;

  const data = open ? info : lastInfo.current;
  const f = buildInfoFigures(data);
  const headMeta = modelName ? `${agentLabel} · ${modelName}` : agentLabel;
  const ctxPair =
    contextUsed != null && contextSize != null && contextSize > 0
      ? `${formatContextCount(contextUsed)} / ${formatContextCount(contextSize)}`
      : contextUsed != null
        ? formatContextCount(contextUsed)
        : "—";
  const pctText = pct == null ? "—" : `${Math.round(pct)}%`;
  const changeText =
    f.changeAdd != null && f.changeDel != null ? `${f.changeAdd} ${f.changeDel}` : "—";
  const turnsText = f.turnsSteps
    ? t("composer.info.turnsSteps", f.turnsSteps)
    : "—";

  const copy = () => {
    const lines = [
      `${t("composer.info.title")} · ${headMeta}`,
      `${t("composer.info.context")}: ${ctxPair} ${pctText}`,
      `${t("composer.info.decode")}: ${f.decode}`,
      `${t("composer.info.prefill")}: ${f.prefill}`,
      `${t("composer.info.firstToken")}: ${f.firstToken}`,
      `${t("composer.info.changes")}: ${changeText}`,
      `${t("composer.info.turns")}: ${turnsText}`,
      `${t("composer.info.cacheHit")}: ${f.cacheHit}`,
      `${t("composer.info.uncachedInput")}: ${f.uncachedInput}`,
      `${t("composer.info.output")}: ${f.output}`,
      `${t("composer.info.cacheRead")}: ${f.cacheRead}`,
      `${t("composer.info.cacheWrite")}: ${f.cacheWrite}`,
    ];
    void navigator.clipboard
      ?.writeText(lines.join("\n"))
      .then(() => setCopied(true))
      .catch(() => undefined);
  };

  return (
    <div
      ref={ref}
      className={`composer-menu composer-menu--info${closing ? " is-closing" : ""}`}
      role="dialog"
      aria-label={t("composer.info.aria")}
      style={closing && frozen ? { ...frozen, right: frozen.right || undefined } : undefined}
    >
      <div className="session-info__head">
        <span className="session-info__title">{t("composer.info.title")}</span>
        <span className="session-info__meta" title={headMeta}>
          {headMeta}
        </span>
        <button
          type="button"
          className="session-info__copy"
          aria-label={t("composer.info.copy")}
          title={copied ? t("composer.info.copied") : t("composer.info.copy")}
          onClick={copy}
        >
          {copied ? <Check size={13} strokeWidth={1.5} /> : <Copy size={13} strokeWidth={1.5} />}
        </button>
      </div>

      <div className="session-info__ctx">
        <span>{t("composer.info.context")}</span>
        <span className="session-info__ctx-pair">{ctxPair}</span>
        <b>{pctText}</b>
      </div>
      <div className={`session-info__bar is-${tone}`} aria-hidden>
        <span style={{ width: `${pct ?? 0}%` }} />
      </div>

      <div className="session-info__sub">{t("composer.info.overview")}</div>
      <div className="session-info__g3">
        <span>
          {t("composer.info.decode")} {f.decode}
        </span>
        <span>
          {t("composer.info.prefill")} {f.prefill}
        </span>
        <span>
          {t("composer.info.firstToken")} {f.firstToken}
        </span>
      </div>
      <div className="session-info__g3">
        <span>
          {t("composer.info.changes")}{" "}
          {f.changeAdd != null && f.changeDel != null ? (
            <>
              <i className="is-add">{f.changeAdd}</i> <i className="is-del">{f.changeDel}</i>
            </>
          ) : (
            "—"
          )}
        </span>
        <span>{turnsText}</span>
        <span>
          {t("composer.info.cacheHit")} {f.cacheHit}
        </span>
      </div>

      <div className="session-info__sub session-info__sub--detail">
        <span>{t("composer.info.detail")}</span>
        {modelName && <span className="session-info__sub-meta">{modelName}</span>}
      </div>
      <div className="session-info__kv">
        <span>{t("composer.info.uncachedInput")}</span>
        <b>{f.uncachedInput}</b>
      </div>
      <div className="session-info__kv">
        <span>{t("composer.info.output")}</span>
        <b>{f.output}</b>
      </div>
      <div className="session-info__kv">
        <span>{t("composer.info.cacheRead")}</span>
        <b>{f.cacheRead}</b>
      </div>
      <div className="session-info__kv">
        <span>{t("composer.info.cacheWrite")}</span>
        <b>{f.cacheWrite}</b>
      </div>
    </div>
  );
}
