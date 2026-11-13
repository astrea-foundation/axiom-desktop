const WARNING_VERSION = "axiom.web-warning.v1:";

export type ConsentStorage = Pick<Storage, "getItem" | "setItem">;

export function browserConsentStorage(): ConsentStorage | null {
  try { return window.localStorage; } catch { return null; }
}

export function skipWebWarning(storage: ConsentStorage | null, accountId: string): boolean {
  try { return storage?.getItem(WARNING_VERSION + encodeURIComponent(accountId)) === "acknowledged"; }
  catch { return false; }
}

export function rememberWebWarning(storage: ConsentStorage | null, accountId: string): void {
  // Failure only means showing the warning next time. This preference is
  // never an authorization token and never automatically enables Web.
  try { storage?.setItem(WARNING_VERSION + encodeURIComponent(accountId), "acknowledged"); }
  catch { /* Keep this confirmation valid without depending on persistence. */ }
}
