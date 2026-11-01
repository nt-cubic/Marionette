/**
 * tokenPrices — API 牌价目录(models.dev)与按桶折算。
 *
 * 牌价是**展示时的查表**,不是记账的一部分:台账只存 token 原始数,
 * 目录拉不到/未收录就安静隐藏金额,后补牌价随时能补算全部历史。
 * 目录 24h 本地缓存(`marionette-token-prices`,只存有价的条目),
 * 拉取失败时退回旧缓存(牌价变动缓慢,陈旧好过没有,更新时间如实展示)。
 *
 * 匹配语义照抄 codeg 的 model-api-rates(实测调教过):厂商自家目录
 * (LAB_PROVIDERS)压过一切转售价;无厂商条目时多家同价才折叠、唯一
 * 优先主机(groq/mistral/bedrock/azure)可单独定价;仍有歧义 = 不定价。
 * 记住:**这是 API 牌价折算,不是订阅实付**——UI 文案必须带上这句话。
 */

export type TokenRate = {
  providerId: string;
  id: string;
  name: string;
  /** USD / 1M tokens;null = 目录未发布该桶价。 */
  input: number | null;
  output: number | null;
  cacheRead: number | null;
  cacheWrite: number | null;
};

export type RateIndex = {
  /** `providerId/modelId`(小写)→ 同 id 的全部目录条目。 */
  byProviderModel: Map<string, TokenRate[]>;
  /** 裸 model id(小写)→ 跨厂商的全部条目。 */
  byModel: Map<string, TokenRate[]>;
  /** 展示名(小写)→ 条目。 */
  byName: Map<string, TokenRate[]>;
  fetchedAt: number | null;
};

/** 只卖自家模型的厂商:其目录价就是牌价,压过任何转售价(codeg 同表)。 */
const LAB_PROVIDERS = [
  "anthropic",
  "openai",
  "google",
  "xai",
  "deepseek",
  "moonshotai",
  "moonshotai-cn",
  "zai",
  "zhipuai",
  "minimax",
  "minimax-cn",
  "xiaomi",
];

/** 无厂商条目时的优先主机;仅当唯一命中时它单独定价。 */
const HOST_PROVIDERS = ["groq", "mistral", "amazon-bedrock", "azure"];

function finiteOrNull(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) && v >= 0 ? v : null;
}

/** 解析 models.dev `api.json` 原文(只保留带任一桶价的模型)。 */
export function buildRateIndex(raw: unknown, fetchedAt: number | null = null): RateIndex {
  const index: RateIndex = {
    byProviderModel: new Map(),
    byModel: new Map(),
    byName: new Map(),
    fetchedAt,
  };
  const root = raw as Record<string, unknown> | null;
  if (!root || typeof root !== "object") return index;
  for (const [providerId, providerRaw] of Object.entries(root)) {
    const provider = providerRaw as { models?: Record<string, unknown> } | null;
    const models = provider?.models;
    if (!models || typeof models !== "object") continue;
    for (const [modelId, modelRaw] of Object.entries(models)) {
      const model = modelRaw as
        | { name?: unknown; cost?: Record<string, unknown> }
        | null;
      const cost = model?.cost;
      if (!cost || typeof cost !== "object") continue;
      const rate: TokenRate = {
        providerId,
        id: modelId,
        name: typeof model?.name === "string" ? model.name : modelId,
        input: finiteOrNull(cost.input),
        output: finiteOrNull(cost.output),
        cacheRead: finiteOrNull(cost.cache_read),
        cacheWrite: finiteOrNull(cost.cache_write),
      };
      if (
        rate.input == null &&
        rate.output == null &&
        rate.cacheRead == null &&
        rate.cacheWrite == null
      ) {
        continue;
      }
      const pk = `${providerId}/${modelId}`.toLowerCase();
      const mk = modelId.toLowerCase();
      const nk = rate.name.toLowerCase();
      for (const [map, key] of [
        [index.byProviderModel, pk],
        [index.byModel, mk],
        [index.byName, nk],
      ] as const) {
        const list = map.get(key) ?? [];
        list.push(rate);
        map.set(key, list);
      }
    }
  }
  return index;
}

/**
 * 多个目录命中折成一个价,或判为歧义不定价:
 * 厂商自家条目直接赢(按 LAB 表序);否则全部同价才折;否则唯一优先主机赢;
 * 其余情况(null)按未定价处理——宁可不算,不给错价。
 */
export function pickRate(rates: TokenRate[]): TokenRate | null {
  if (rates.length === 0) return null;
  if (rates.length === 1) return rates[0] ?? null;
  for (const lab of LAB_PROVIDERS) {
    const hit = rates.find((r) => r.providerId === lab);
    if (hit) return hit;
  }
  const first = rates[0]!;
  const allAgree = rates.every(
    (r) =>
      r.input === first.input &&
      r.output === first.output &&
      r.cacheRead === first.cacheRead &&
      r.cacheWrite === first.cacheWrite,
  );
  if (allAgree) return first;
  const hostHits = rates.filter((r) => HOST_PROVIDERS.includes(r.providerId));
  if (hostHits.length === 1) return hostHits[0] ?? null;
  return null;
}

/** Marionette 模型 id 的命名空间剥离:取「真模型 id」与(若有)厂商段。 */
function modelIdParts(modelId: string): { bare: string; provider: string | null; full: string } {
  let full = modelId.trim();
  // zcode 桥 `marionette\<id>` / dsh 桥 `marionette::provider/model`
  if (full.includes("\\")) full = full.slice(full.lastIndexOf("\\") + 1);
  if (full.includes("::")) full = full.slice(full.lastIndexOf("::") + 2);
  full = full.toLowerCase();
  const slash = full.indexOf("/");
  if (slash > 0) {
    return { bare: full.slice(slash + 1), provider: full.slice(0, slash), full };
  }
  return { bare: full, provider: null, full };
}

/** 查一个 Marionette 模型 id 的牌价;未收录/歧义 → null(不编价)。 */
export function rateForModel(modelId: string, index: RateIndex): TokenRate | null {
  const { bare, provider, full } = modelIdParts(modelId);
  if (provider) {
    const scoped = index.byProviderModel.get(full);
    if (scoped && scoped.length > 0) return pickRate(scoped);
  }
  const byId = index.byModel.get(bare);
  if (byId && byId.length > 0) return pickRate(byId);
  const byFull = index.byModel.get(full);
  if (byFull && byFull.length > 0) return pickRate(byFull);
  return null;
}

export type BucketCost = {
  /** null = 一个可计价模型都没匹配上。 */
  usd: number | null;
  pricedTokens: number;
  unpricedTokens: number;
};

function priceBucket(
  tokens: number,
  perMillion: number | null,
): { usd: number; priced: number; unpriced: number } {
  if (tokens <= 0) return { usd: 0, priced: 0, unpriced: 0 };
  if (perMillion == null) return { usd: 0, priced: 0, unpriced: tokens };
  return { usd: (tokens / 1_000_000) * perMillion, priced: tokens, unpriced: 0 };
}

/** 一行模型用量 × 牌价 → 费用与覆盖率(未定价 token 如实计入分母)。 */
export function costOfBuckets(
  row: { input: number; output: number; cached: number; cacheCreation: number },
  rate: TokenRate | null,
): BucketCost {
  const total =
    row.input + row.output + row.cached + row.cacheCreation;
  if (!rate) return { usd: null, pricedTokens: 0, unpricedTokens: total };
  const input = priceBucket(row.input, rate.input);
  const output = priceBucket(row.output, rate.output);
  const cacheRead = priceBucket(row.cached, rate.cacheRead);
  const cacheWrite = priceBucket(row.cacheCreation, rate.cacheWrite);
  return {
    usd: input.usd + output.usd + cacheRead.usd + cacheWrite.usd,
    pricedTokens: input.priced + output.priced + cacheRead.priced + cacheWrite.priced,
    unpricedTokens: input.unpriced + output.unpriced + cacheRead.unpriced + cacheWrite.unpriced,
  };
}

// ── 目录获取(24h 缓存,失败退旧缓存) ─────────────────────────────────

const PRICE_KEY = "marionette-token-prices";
const PRICE_TTL_MS = 24 * 60 * 60 * 1000;
const CATALOG_URL = "https://models.dev/api.json";

type PriceCache = { fetchedAt: number; rates: TokenRate[] };

function readCache(): PriceCache | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(PRICE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as PriceCache;
    if (!parsed || !Array.isArray(parsed.rates) || typeof parsed.fetchedAt !== "number") {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

function writeCache(cache: PriceCache): void {
  try {
    window.localStorage.setItem(PRICE_KEY, JSON.stringify(cache));
  } catch {
    /* 配额满/被拒:缓存是优化不是依赖,跳过 */
  }
}

let inflight: Promise<RateIndex> | null = null;

/** 空目录(测试与「目录不可用」的兜底形状)。 */
export function emptyRateIndex(): RateIndex {
  return { byProviderModel: new Map(), byModel: new Map(), byName: new Map(), fetchedAt: null };
}

/**
 * 取牌价目录(并发去重)。缓存 24h 内直接用;过期重拉;重拉失败退回
 * 旧缓存(无论多旧——牌价慢变,陈旧好过没有);两边都没有 → 空目录,
 * UI 据此隐藏金额区。**订阅/中转的用户看到的金额是牌价折算,不是实付。**
 */
export function fetchPriceCatalog(force = false): Promise<RateIndex> {
  if (!force && inflight) return inflight;
  const cached = readCache();
  if (!force && cached && Date.now() - cached.fetchedAt < PRICE_TTL_MS) {
    return Promise.resolve(indexFromCache(cached));
  }
  inflight = (async () => {
    try {
      const res = await fetch(CATALOG_URL, { headers: { Accept: "application/json" } });
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const raw = (await res.json()) as unknown;
      const fresh = buildRateIndex(raw, Date.now());
      if (fresh.byModel.size > 0) {
        const rates: TokenRate[] = [...fresh.byModel.values()].flat();
        writeCache({ fetchedAt: Date.now(), rates });
        return fresh;
      }
      throw new Error("catalog parsed empty");
    } catch {
      if (cached) return indexFromCache(cached);
      return emptyRateIndex();
    } finally {
      inflight = null;
    }
  })();
  return inflight;
}

function indexFromCache(cache: PriceCache): RateIndex {
  const index = emptyRateIndex();
  index.fetchedAt = cache.fetchedAt;
  for (const rate of cache.rates) {
    const pk = `${rate.providerId}/${rate.id}`.toLowerCase();
    const mk = rate.id.toLowerCase();
    const nk = rate.name.toLowerCase();
    for (const [map, key] of [
      [index.byProviderModel, pk],
      [index.byModel, mk],
      [index.byName, nk],
    ] as const) {
      const list = map.get(key) ?? [];
      list.push(rate);
      map.set(key, list);
    }
  }
  return index;
}

/** 金额格式:$1,234.56 / $0.000481(<$0.01 保留足够位)。 */
export function formatUsd(usd: number): string {
  if (usd > 0 && usd < 0.01) return `$${usd.toFixed(6).replace(/0+$/, "").replace(/\.$/, "")}`;
  return `$${usd.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
}
