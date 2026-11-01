import { useEffect } from "react";
import { useT } from "../lib/i18n";
import { UsageAnalysisBody } from "./UsageAnalysisDialog";

/**
 * 「Token 用量」伪标签页 / 贴边整页。挂载点的铬负责标题与关闭
 * （标签 × / 贴边「返回」）；本页只铺内容。Esc 关闭，已处理的
 * Esc（格子 tooltip、柱扫读）不冒泡到这里。
 */
export function UsagePage({ onClose }: { onClose: () => void }) {
  const t = useT();
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="usage-page" role="region" aria-label={t("usageLedger.analysisTitle")}>
      <div className="usage-page__scroll">
        <UsageAnalysisBody />
      </div>
    </div>
  );
}
