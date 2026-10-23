import type { BridgeEntryView, BridgeScanReport } from "./api";

type AgentScan = { agent: string; entries: BridgeEntryView[]; note?: string | null; error?: string | null };

/** Normalize the actual Rust ScanReport, retaining read failures and references. */
export function normalizeCredentialScan(raw: unknown): BridgeScanReport {
  if (!raw || typeof raw !== "object") throw new Error("凭据扫描返回了无效结果");
  if ("sources" in raw && Array.isArray(raw.sources)) return raw as BridgeScanReport;
  const scans = Object.values(raw).filter((row): row is AgentScan => row != null && typeof row === "object" && typeof row.agent === "string" && Array.isArray(row.entries));
  if (!scans.length) throw new Error("凭据扫描返回了未知格式");
  return {
    sources: scans.map(row => ({ agent: row.agent, status: row.error ? "restricted" : row.entries.length ? "found" : row.note?.includes("存储") ? "restricted" : "missing", detail: row.error || row.note || null })),
    entries: scans.flatMap(row => row.entries),
  };
}
