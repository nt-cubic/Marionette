import { ChevronDown, ChevronUp, EyeOff } from "lucide-react";
import { useState } from "react";
import { agentInkStyle } from "../../lib/agentInk";
import { useT } from "../../lib/i18n";
import type { ImageSupport } from "../../lib/imageSupport";
import {
  countSeenBySource,
  estimateSeenTokens,
  filterSeen,
  formatSeenTime,
  groupSeenByAge,
  type SeenFilter,
  type SeenGroup,
  type SeenImage,
} from "../../lib/seenImages";
import { fmtTokens } from "../usage/primitives";
import { SeenThumb } from "./SeenThumb";

const OPEN_KEY = "marionette-seen-images-open";
const PAGE = 60;

function readStoredOpen(): boolean {
  try {
    return window.localStorage.getItem(OPEN_KEY) === "1";
  } catch {
    return false; // storage blocked -> stay collapsed
  }
}

/** Trim the age groups to at most `cap` thumbnails; a label never appears with zero items. */
function capGroups(groups: readonly SeenGroup[], cap: number): { groups: SeenGroup[]; shown: number } {
  const capped: SeenGroup[] = [];
  let shown = 0;
  for (const group of groups) {
    if (shown >= cap) break;
    const items = group.items.slice(0, cap - shown);
    shown += items.length;
    capped.push({ ...group, items });
  }
  return { groups: capped, shown };
}

/** The "model cannot take images" note: faint eye-off icon + sentence + optional switch button. */
function OffNote({ text, onSwitch }: { text: string; onSwitch?: () => void }): JSX.Element {
  const t = useT();
  return (
    <div className="seen-rail__offnote">
      <EyeOff size={13} strokeWidth={1.5} aria-hidden="true" />
      <span>
        {text}
        {onSwitch && (
          <>
            {" "}
            <button type="button" className="seen-rail__switch" onClick={onSwitch}>
              {t("seenImages.switchModel")}
            </button>
          </>
        )}
      </span>
    </div>
  );
}

export function SeenImagesSection(props: {
  images: SeenImage[];
  support: ImageSupport;
  notSentText?: string | null;
  onSwitchModel?: () => void;
  agentId?: string;
  truncated: boolean;
  onOpen: (index: number) => void;
}): JSX.Element | null {
  const { images, support, notSentText, onSwitchModel, agentId, truncated, onOpen } = props;
  const t = useT();
  const [open, setOpen] = useState<boolean>(readStoredOpen);
  const [filter, setFilter] = useState<SeenFilter>("all");
  const [cap, setCap] = useState<number>(PAGE);
  const off = support === "no";
  const counts = countSeenBySource(images);

  if (images.length === 0) return null;

  const applyOpen = (next: boolean) => {
    setOpen(next);
    try {
      window.localStorage.setItem(OPEN_KEY, next ? "1" : "0");
    } catch {
      // storage refused: the in-memory state still applies
    }
  };

  const tip = (image: SeenImage): string =>
    `${t("seenImages.tip", { via: t(`seenImages.via.${image.via}`), time: formatSeenTime(image.at) })}\n${image.name}`;

  const filtered = filterSeen(images, filter);
  const { groups, shown } = capGroups(groupSeenByAge(filtered), cap);
  const filters: { key: SeenFilter; total: number }[] = [
    { key: "all", total: counts.all },
    { key: "you", total: counts.you },
    { key: "tool", total: counts.tool },
  ];

  return (
    <section className="rail-sec seen-rail" aria-label={t("seenImages.title")} style={agentInkStyle(agentId)}>
      <div className="rail-sh">
        <span>{t("seenImages.title")}</span>
        <span className="rail-sh__meta">
          {t("seenImages.meta", { total: images.length, tokens: fmtTokens(estimateSeenTokens(images.length)) })}
        </span>
        <span className="rail-act">
          <button
            type="button"
            className="rail-ib"
            aria-expanded={open}
            aria-label={open ? t("seenImages.rail.collapse") : t("seenImages.rail.expand")}
            onClick={() => applyOpen(!open)}
          >
            {open ? (
              <ChevronUp size={14} strokeWidth={1.5} aria-hidden="true" />
            ) : (
              <ChevronDown size={14} strokeWidth={1.5} aria-hidden="true" />
            )}
          </button>
        </span>
      </div>

      {!open && (
        <>
          <div className="seen-rail__row">
            {images.slice(0, 4).map((image, index) => (
              <SeenThumb
                key={image.key}
                image={image}
                width={56}
                height={42}
                dim={off}
                title={tip(image)}
                onClick={() => onOpen(index)}
              />
            ))}
            {images.length > 4 && (
              <button
                type="button"
                className="seen-rail__more"
                onClick={() => applyOpen(true)}
                aria-label={t("seenImages.rail.moreTile", { n: images.length - 4 })}
              >
                +{images.length - 4}
              </button>
            )}
          </div>
          {off && notSentText && <OffNote text={notSentText} onSwitch={onSwitchModel} />}
        </>
      )}

      {open && (
        <>
          <div className="seen-rail__seg" role="tablist" aria-label={t("seenImages.title")}>
            {filters.map(({ key, total }) => (
              <button
                key={key}
                type="button"
                role="tab"
                aria-selected={filter === key}
                className={filter === key ? "is-on" : ""}
                onClick={() => setFilter(key)}
              >
                {t(`seenImages.filter.${key}`, { total })}
              </button>
            ))}
          </div>
          {off && notSentText && <OffNote text={notSentText} onSwitch={onSwitchModel} />}
          {filtered.length === 0 ? (
            <div className="seen-rail__empty">{t("seenImages.rail.empty")}</div>
          ) : (
            groups.map((group) => (
              <div key={group.key}>
                <div className="seen-rail__gl">
                  {/* groupSeenByAge always fills month/day for kind "day"; the ?? 0 only satisfies the type */}
                  {group.kind === "day"
                    ? t("seenImages.group.day", { month: group.month ?? 0, day: group.day ?? 0 })
                    : t(`seenImages.group.${group.kind}`)}
                </div>
                <div className="seen-rail__grid">
                  {group.items.map((image) => (
                    <SeenThumb
                      key={image.key}
                      image={image}
                      width={84}
                      height={64}
                      showTime
                      dim={off}
                      title={tip(image)}
                      onClick={() => onOpen(images.indexOf(image))}
                    />
                  ))}
                </div>
              </div>
            ))
          )}
          {filtered.length > shown && (
            <div className="seen-rail__morebar">
              <button type="button" onClick={() => setCap((prev) => prev + PAGE)}>
                {t("seenImages.rail.more", { shown, total: filtered.length })}
              </button>
            </div>
          )}
          {truncated && <div className="seen-rail__foot">{t("seenImages.partial")}</div>}
        </>
      )}
    </section>
  );
}
