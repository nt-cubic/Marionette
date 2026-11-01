import { useEffect } from "react";
import { useT } from "../lib/i18n";
import { UsageAnalysisBody } from "./UsageAnalysisDialog";

/**
 * 「Token 用量」伪标签页(2026-11-01 产品定案:与收纳盒/圆桌/浏览器同级,
 * 不再只是弹窗)。挂载在 `.usage-tab-page`(council 同模式):只切换中栏
 * 那一列,左右侧栏照常;Esc / × / Ctrl+W 关闭标签。内容本体与弹窗形态
 * (贴边/detached 兜底)共用 UsageAnalysisBody。
 */
export function UsagePage({ onClose }: { onClose: () => void }) {
  const t = useT();
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="usage-page" role="region" aria-label={t("usageLedger.analysisTitle")}>
      <div className="usage-page__bar">
        <span className="usage-page__title">{t("usageLedger.analysisTitle")}</span>
        <button
          type="button"
          className="pill-action pill-action--icon"
          aria-label={t("common.close")}
          onClick={onClose}
        >
          <span aria-hidden>✕</span>
        </button>
      </div>
      <div className="usage-page__scroll">
        <UsageAnalysisBody />
      </div>
    </div>
  );
}
