export function usdFromMicrousd(value: number, maximumFractionDigits = 6): string {
  return new Intl.NumberFormat(undefined, {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits,
  }).format(value / 1_000_000);
}
