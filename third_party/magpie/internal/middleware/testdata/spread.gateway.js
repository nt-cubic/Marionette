// spread.gateway.js: the example on usemagpie.ai/docs/plugins#gateway-points.
// Routing "spread/fewest-tokens" tries the accounts or keys that used the
// fewest tokens lately first, resting ones last; onUsage says in magpie's
// log which requests were slow.

export const strategies = {
  "fewest-tokens": (pool, seats) =>
    seats.map((_, i) => i).sort((a, b) =>
      (seats[a].resting - seats[b].resting) || (seats[a].tokens - seats[b].tokens)),
};

let slow = 0;

export function onUsage(row, options) {
  if (row.ms > (options.slowMs ?? 30000)) {
    slow++;
    console.log(`slow: ${row.provider} ${row.model} ${row.ms} ms (${slow} so far)`);
  }
}
