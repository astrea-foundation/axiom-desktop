import type { CryptoCurrency } from "@axiom/axiom-acp-client";

const icons: Record<string, string> = {
  btc: new URL("../brand/crypto/btc.svg", import.meta.url).href,
  eth: new URL("../brand/crypto/eth.svg", import.meta.url).href,
  ltc: new URL("../brand/crypto/ltc.svg", import.meta.url).href,
  sol: new URL("../brand/crypto/sol.svg", import.meta.url).href,
  usdc: new URL("../brand/crypto/usdc.svg", import.meta.url).href,
  usdt: new URL("../brand/crypto/usdt.svg", import.meta.url).href,
  xmr: new URL("../brand/crypto/xmr.svg", import.meta.url).href,
};
const names: Record<string, string> = { btc: "Bitcoin", eth: "Ethereum", ltc: "Litecoin", sol: "Solana", xmr: "Monero" };
const networks: Record<string, string> = { btc: "Bitcoin", eth: "Ethereum", ltc: "Litecoin", sol: "Solana", trx: "Tron (TRC20)", xmr: "Monero" };

export function cryptoSymbol(code: string): string {
  return code.startsWith("usdc") ? "USDC" : code.startsWith("usdt") ? "USDT" : code.toUpperCase();
}

export function cryptoName(coin: CryptoCurrency): string {
  return names[coin.code] ?? (coin.code.startsWith("usdc") ? "USDC" : coin.code.startsWith("usdt") ? "USDT" : coin.name);
}

export function cryptoNetwork(coin: CryptoCurrency): string {
  return networks[coin.network.toLowerCase()] ?? coin.network;
}

/** Bundled artwork keeps asset discovery and QR rendering local. */
export function CryptoIcon({ code, className = "h-7 w-7" }: { code: string; className?: string }) {
  const asset = code.startsWith("usdc") ? "usdc" : code.startsWith("usdt") ? "usdt" : code;
  const source = icons[asset];
  return source ? <img src={source} alt="" aria-hidden="true" draggable={false} className={`shrink-0 rounded-full ${className}`} />
    : <span aria-hidden="true" className={`inline-flex shrink-0 items-center justify-center rounded-full bg-[var(--wash-chip)] text-[10px] font-medium ${className}`}>{cryptoSymbol(code).slice(0, 2)}</span>;
}
