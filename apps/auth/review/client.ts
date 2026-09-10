// Selected ONLY by vite.review.config.ts. No live requests or stored credentials.
import { ApiError } from "../src/api/client";
import type { AccountSummary, IdentitySummary, NativeAuthorizationSummary, PaymentInvoice, SessionResponse } from "../src/api/types";
export { ApiError, AuthApiClient, safeReturnPath, normalizeApiOrigin, apiOriginForHost, allowedGoogleAuthorizationUrl } from "../src/api/client";
const params = new URLSearchParams(window.location.search);
const view = params.get("view");
const account: AccountSummary = { id: "sample-account", display_name: "Alex Morgan", avatar_url: null, verified_email: "alex@example.com", linked_methods: ["google", "passkey"] };
let signedIn = ["/account", "/native/authorize", "/payments/return"].includes(window.location.pathname) || params.get("intent") === "link";
let identities: IdentitySummary[] = [
  { id: "sample-google", kind: "google", label: "alex@example.com", summary: null, created_at: "2026-09-01T12:00:00Z", last_used_at: null },
  { id: "sample-passkey", kind: "passkey", label: "MacBook Pro", summary: null, created_at: "2026-09-01T12:00:00Z", last_used_at: null },
];
const wait = async () => new Promise<void>((resolve) => setTimeout(resolve, 220));
const ok = async () => { await wait(); if (view === "error") throw new ApiError(503); return { ok: true }; };
const login = async () => { await ok(); signedIn = true; return { ok: true }; };
const invoice = (): PaymentInvoice => ({ id: "sample-invoice", amount_usd_cents: 2500, state: view === "paid" ? "paid" : view === "review" ? "review" : view === "expired" ? "expired" : "pending", expires_at: "2026-09-11T12:00:00Z", paid_at: view === "paid" ? "2026-09-10T12:00:00Z" : null });
export const authApi = {
  async session(): Promise<SessionResponse> { if (view === "loading") return new Promise(() => {}); await wait(); if (view === "unavailable") throw new ApiError(503); return signedIn ? { authenticated: true, account } : { authenticated: false }; },
  accountFromSession: (session: SessionResponse) => session.authenticated ? session.account ?? null : null,
  passwordSignup: ok,
  passwordLogin: async () => { await ok(); return {authenticated: false as const, challenge_id: "b".repeat(32), expires_at: new Date(Date.now() + 600000).toISOString(), resend_after_seconds: 60}; },
  resendLoginCode: async () => { await ok(); return {authenticated: false as const, challenge_id: "c".repeat(32), expires_at: new Date(Date.now() + 600000).toISOString(), resend_after_seconds: 60}; },
  verifyLoginCode: async (_challengeId: string, code: string) => { await ok(); if (code !== "123456") throw new ApiError(401); signedIn = true; return {authenticated: true, account}; },
  requestEmailVerification: ok, requestPasswordReset: ok, resetPassword: ok, verifyEmail: ok,
  googleStartUrl: (returnTo: string) => returnTo === "/account" ? "/account" : returnTo,
  googleLinkStart: async () => "/account",
  passkeyAuthenticationOptions: async () => { throw new DOMException("Preview", "NotAllowedError"); },
  passkeyRegistrationOptions: async () => { throw new DOMException("Preview", "NotAllowedError"); },
  verifyPasskeyAuthentication: login, verifyPasskeyRegistration: ok,
  siweChallenge: async () => { throw new DOMException("Preview", "NotAllowedError"); }, verifySiwe: login,
  identities: async () => { await wait(); return identities; },
  removeIdentity: async (id: string) => { await ok(); identities = identities.filter((entry) => entry.id !== id); return { ok: true }; },
  logout: async () => { signedIn = false; return ok(); }, logoutAll: async () => { signedIn = false; return ok(); }, deleteAccount: async () => { signedIn = false; return ok(); },
  billingStatus: async () => { await wait(); return { available_microusd: 12400000, posted_microusd: 12400000, held_microusd: 0, ledger_sequence: 3, currency: "microUSD" as const, invoices_enabled: false, invoice_min_usd_cents: 100, invoice_max_usd_cents: 10000, settlement_asset: null, payment_account: { network: "mainnet" as const, asset: "ZEC" as const, conversion_status: "none" as const, valuation_enabled: true } }; },
  invoice: async () => { await wait(); return invoice(); }, createInvoice: async () => { await wait(); return invoice(); },
  nativeAuthorization: async (): Promise<NativeAuthorizationSummary> => { await wait(); return { id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", status: view === "approved" ? "approved" : view === "expired" ? "expired" : "pending", user_code: "K7MT-P2RV", client_name: "Axiom Desktop", client_platform: "macOS · Your computer", expires_at: "2026-09-11T12:00:00Z" }; },
  approveNativeAuthorization: async () => { await ok(); return { status: "approved" as const }; },
};
