/**
 * UsageLegend — Token 用量页 v2 末尾的口径脚注:三栏说明数字的来源(实测 / 估算+推算 / 折算)。
 */
import { useT } from "../../lib/i18n";
import { SourceTag } from "./primitives";

export function UsageLegend() {
  const t = useT();
  return (
    <footer className="uv2-legend">
      <div>
        <SourceTag source="measured" />
        <span>{t("usageV2.legend.measured")}</span>
        <p className="uv2-legend__sub">{t("usageV2.legend.measuredSub")}</p>
      </div>
      <div>
        <SourceTag source="estimated" />
        <span>{t("usageV2.legend.estimated")}</span>
        <SourceTag source="derived" />
        <span>{t("usageV2.legend.derived")}</span>
        <p className="uv2-legend__sub">{t("usageV2.legend.estimatedSub")}</p>
      </div>
      <div>
        <SourceTag source="priced" />
        <span>{t("usageV2.legend.priced")}</span>
        <p className="uv2-legend__sub">{t("usageV2.legend.pricedSub")}</p>
      </div>
    </footer>
  );
}
