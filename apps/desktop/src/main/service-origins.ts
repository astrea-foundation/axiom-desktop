import type { ClientState } from "@axiom/axiom-acp-client";

export const PRODUCTION_AUTH_ORIGIN = "https://auth.axiom.stream";
export const PRODUCTION_API_ORIGIN = "https://api.axiom.stream";
export const STAGING_AUTH_ORIGIN = "https://auth-staging.axiom.stream";
export const STAGING_API_ORIGIN = "https://api-staging.axiom.stream";

export interface ServiceOrigins {
  auth: string;
  api: string;
}

type OriginEnvironment = {
  packaged: boolean;
  authUrl?: string;
  apiUrl?: string;
};

type OriginClass = "production" | "staging" | "loopback";

function normalizedOrigin(value: string, label: string): { origin: string; kind: OriginClass } {
  const url = new URL(value);
  if (url.username || url.password || url.pathname !== "/" || url.search || url.hash) {
    throw new Error(`${label} URL must be an origin without credentials, path, query, or fragment`);
  }
  if (url.origin === (label === "Authentication" ? PRODUCTION_AUTH_ORIGIN : PRODUCTION_API_ORIGIN)) {
    return { origin: url.origin, kind: "production" };
  }
  if (url.origin === (label === "Authentication" ? STAGING_AUTH_ORIGIN : STAGING_API_ORIGIN)) {
    return { origin: url.origin, kind: "staging" };
  }
  const loopback =
    (url.protocol === "https:" || (label === "Authentication" && url.protocol === "http:")) &&
    (url.hostname === "localhost" || url.hostname === "127.0.0.1" || url.hostname === "[::1]");
  if (!loopback) {
    throw new Error(`${label} URL must use the matching Axiom deployment or a loopback origin${label === "API" ? " with HTTPS" : ""}`);
  }
  return { origin: url.origin, kind: "loopback" };
}

/** Resolve one coherent API/auth audience before the sidecar starts. */
export function resolveServiceOrigins(environment: OriginEnvironment): ServiceOrigins {
  if (environment.packaged) {
    return { auth: PRODUCTION_AUTH_ORIGIN, api: PRODUCTION_API_ORIGIN };
  }
  const auth = normalizedOrigin(environment.authUrl ?? PRODUCTION_AUTH_ORIGIN, "Authentication");
  const api = normalizedOrigin(environment.apiUrl ?? PRODUCTION_API_ORIGIN, "API");
  if (auth.kind !== api.kind) {
    throw new Error("AXIOM_AUTH_URL and AXIOM_BASE_URL must select the same deployment");
  }
  return { auth: auth.origin, api: api.origin };
}

export function trustedNativeAuthorizationUrl(value: string, authOrigin: string): string {
  const target = new URL(value);
  const entries = [...target.searchParams.entries()];
  const authorizationIds = target.searchParams.getAll("authorization_id");
  const states = target.searchParams.getAll("state");
  if (
    target.origin !== authOrigin ||
    target.username ||
    target.password ||
    target.pathname !== "/native/authorize" ||
    target.hash ||
    entries.length !== 2 ||
    authorizationIds.length !== 1 ||
    !/^[0-9a-f]{32}$/.test(authorizationIds[0] ?? "") ||
    states.length !== 1 ||
    !/^[A-Za-z0-9_-]{32,128}$/.test(states[0] ?? "")
  ) {
    throw new Error("AxiomCLI returned an untrusted native authorization URL");
  }
  return target.toString();
}

export function requirePaymentAccount(
  accountId: string,
  state: Pick<ClientState, "connected" | "account">,
): void {
  if (!state.connected || state.account?.state !== "valid" || !accountId
    || state.account.account?.id !== accountId) {
    throw new Error("The account changed. Sign in again before topping up.");
  }
}
