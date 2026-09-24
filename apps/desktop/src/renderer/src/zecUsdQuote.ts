import type { ZecUsdQuote } from "@axiom/axiom-acp-client";

/** A display quote expires independently of billing refreshes and connection state. */
export function currentZecUsdQuote(quote: ZecUsdQuote | null | undefined, now: number): ZecUsdQuote | null {
  if (!quote || !/^[1-9][0-9]{0,12}$/.test(quote.price_microusd_per_zec)) return null;
  const price = Number(quote.price_microusd_per_zec);
  const asOf = Date.parse(quote.as_of);
  const expires = Date.parse(quote.expires_at);
  return price <= 1_000_000_000_000
    && (quote.source === "coinbase" || quote.source === "kraken")
    && asOf <= now && now < expires && expires - asOf === 60_000
    ? quote : null;
}
