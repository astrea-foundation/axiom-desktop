const SHIELDZ_PAY_ORIGIN = "https://shieldz.cash";
const SHIELDZ_PAY_PATH = /^\/pay\/[A-Za-z0-9_-]{1,160}$/;

export function allowedShieldzPayUrl(value: string): string {
  const url = new URL(value);
  if (
    url.origin !== SHIELDZ_PAY_ORIGIN ||
    !SHIELDZ_PAY_PATH.test(url.pathname) ||
    url.username ||
    url.password ||
    url.port ||
    url.search ||
    url.hash
  ) {
    throw new TypeError("Invalid Shieldz payment URL");
  }
  return url.toString();
}
