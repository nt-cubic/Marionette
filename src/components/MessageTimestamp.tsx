import { useMemo } from "react";

type MessageTimestampProps = {
  createdAt: string;
};

/**
 * Formats a message timestamp for card display.
 *
 * Rules:
 *  - Same day    → time only  ("2:34 PM")
 *  - Yesterday   → label      ("Yesterday")
 *  - 2–6 days    → day name   ("Mon")
 *  - 7+ / this yr→ month+day  ("Jul 15")
 *  - Prev years  → full date  ("Jul 15, 2024")
 *  - Hover       → tooltip    ("2025/7/26 14:34:23")
 */
function formatTime(iso: string): { display: string; tooltip: string } {
  const date = new Date(iso);
  const now = new Date();
  const tooltip = date.toLocaleString();

  try {
    // Same calendar day → time only
    if (date.toDateString() === now.toDateString()) {
      return {
        display: date.toLocaleTimeString(undefined, {
          hour: "numeric",
          minute: "2-digit",
          hour12: true,
        }),
        tooltip,
      };
    }

    const ms = now.getTime() - date.getTime();
    const days = Math.floor(ms / (1000 * 60 * 60 * 24));

    // Yesterday
    if (days === 1) {
      return { display: "Yesterday", tooltip };
    }

    // 2–6 days ago → short weekday
    if (days >= 2 && days <= 6) {
      return {
        display: date.toLocaleDateString(undefined, { weekday: "short" }),
        tooltip,
      };
    }

    // Same calendar year → month + day
    if (date.getFullYear() === now.getFullYear()) {
      return {
        display: date.toLocaleDateString(undefined, {
          month: "short",
          day: "numeric",
        }),
        tooltip,
      };
    }

    // Previous year(s) → full date
    return {
      display: date.toLocaleDateString(undefined, {
        month: "short",
        day: "numeric",
        year: "numeric",
      }),
      tooltip,
    };
  } catch {
    // Fallback for invalid dates
    return { display: iso, tooltip: iso };
  }
}

export function MessageTimestamp({ createdAt }: MessageTimestampProps) {
  const { display, tooltip } = useMemo(() => formatTime(createdAt), [createdAt]);

  return (
    <time className="event-card__timestamp" dateTime={createdAt} title={tooltip}>
      {display}
    </time>
  );
}
