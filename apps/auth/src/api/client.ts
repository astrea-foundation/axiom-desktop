import type {
  AccountSummary,
  AuthIntent,
  BillingStatus,
  IdentitySummary,
  EmailLoginChallenge,
  NativeApprovalResult,
  NativeAuthorizationSummary,
  PasskeyCreationOptions,
  PasskeyRequestOptions,
  PaymentInvoice,
  PublicResult,
  SessionResponse,
  SiweChallenge,
} from "./types";

const DEFAULT_API_ORIGIN = import.meta.env.VITE_API_ORIGIN ?? apiOriginForHost(
  typeof window === "undefined" ? "auth.axiom.stream" : window.location.hostname,
);
const MAX_RESPONSE_BYTES = 64 * 1024;
const DEFAULT_TIMEOUT_MS = 15_000;

type CsrfPolicy = "never" | "if-present" | "required";

interface RequestOptions {
  method?: "GET" | "POST" | "DELETE";
  body?: unknown;
  csrf?: CsrfPolicy;
  signal?: AbortSignal;
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code = "request_failed") {
    super(publicErrorMessage(status, code));
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

export class AuthApiClient {
  readonly origin: string;
  #csrfToken: string | null = null;

  constructor(origin = DEFAULT_API_ORIGIN) {
    this.origin = normalizeApiOrigin(origin);
  }

  async session(signal?: AbortSignal): Promise<SessionResponse> {
    const session = await this.#request<SessionResponse>("/api/v1/auth/session", { signal });
    this.#csrfToken = session.authenticated ? (session.csrf_token ?? null) : null;
    return session;
  }

  async passwordSignup(input: {
    email: string;
    display_name?: string;
    intent?: Extract<AuthIntent, "signup" | "link">;
  }): Promise<PublicResult> {
    const link = input.intent === "link";
    return this.#request(link ? "/api/v1/auth/password/link" : "/api/v1/auth/password/signup", {
      method: "POST",
      body: { email: input.email, display_name: input.display_name },
      csrf: link ? "required" : "never",
    });
  }

  async passwordLogin(email: string, password: string): Promise<EmailLoginChallenge> {
    const result = await this.#request<EmailLoginChallenge>("/api/v1/auth/password/login", {
      method: "POST",
      body: { email, password },
      csrf: "never",
    });
    return emailLoginChallenge(result);
  }

  async resendLoginCode(challengeId: string): Promise<EmailLoginChallenge> {
    const result = await this.#request<EmailLoginChallenge>("/api/v1/auth/password/login/resend", {
      method: "POST", body: { challenge_id: challengeId }, csrf: "never",
    });
    return emailLoginChallenge(result);
  }

  async verifyLoginCode(challengeId: string, code: string): Promise<SessionResponse> {
    const session = await this.#request<SessionResponse>("/api/v1/auth/password/login/verify", {
      method: "POST", body: { challenge_id: challengeId, code }, csrf: "never",
    });
    if (session.authenticated !== true || !session.csrf_token) throw new ApiError(0, "invalid_response");
    this.#csrfToken = session.csrf_token;
    return session;
  }

  async requestEmailVerification(email: string): Promise<PublicResult> {
    return this.#request("/api/v1/auth/password/verify/request", {
      method: "POST",
      body: { email },
      csrf: "never",
    });
  }

  async verifyEmail(token: string, password: string): Promise<PublicResult> {
    return this.#request("/api/v1/auth/password/verify", {
      method: "POST",
      body: { token, new_password: password },
      csrf: "if-present",
    });
  }

  async requestPasswordReset(email: string): Promise<PublicResult> {
    return this.#request("/api/v1/auth/password/reset/request", {
      method: "POST",
      body: { email },
      csrf: "never",
    });
  }

  async resetPassword(token: string, password: string): Promise<PublicResult> {
    return this.#request("/api/v1/auth/password/reset", {
      method: "POST",
      body: { token, new_password: password },
      csrf: "never",
    });
  }

  async passkeyAuthenticationOptions(): Promise<PasskeyRequestOptions> {
    return this.#request("/api/v1/auth/passkeys/authentication/options", {
      method: "POST",
      body: { intent: "signin" },
      csrf: "never",
    });
  }

  async verifyPasskeyAuthentication(
    challengeId: string,
    credential: unknown,
  ): Promise<PublicResult> {
    return this.#request("/api/v1/auth/passkeys/authentication/verification", {
      method: "POST",
      body: { challenge_id: challengeId, credential },
      csrf: "never",
    });
  }

  async passkeyRegistrationOptions(
    intent: Extract<AuthIntent, "signup" | "link">,
    displayName?: string,
  ): Promise<PasskeyCreationOptions> {
    return this.#request("/api/v1/auth/passkeys/registration/options", {
      method: "POST",
      body: { display_name: displayName || undefined },
      csrf: intent === "link" ? "required" : "never",
    });
  }

  async verifyPasskeyRegistration(
    challengeId: string,
    credential: unknown,
    intent: Extract<AuthIntent, "signup" | "link">,
    label?: string,
  ): Promise<PublicResult> {
    return this.#request("/api/v1/auth/passkeys/registration/verification", {
      method: "POST",
      body: { challenge_id: challengeId, credential, device_label: label || undefined },
      csrf: intent === "link" ? "required" : "never",
    });
  }

  googleStartUrl(returnTo: string): string {
    const url = this.#url("/api/v1/auth/google/start");
    url.searchParams.set("return_to", safeReturnPath(returnTo));
    return url.toString();
  }

  async googleLinkStart(returnTo: string): Promise<string> {
    const result = await this.#request<{ authorization_url: string }>(
      `/api/v1/auth/google/link/start?return_to=${encodeURIComponent(safeReturnPath(returnTo))}`,
      { method: "POST", body: {}, csrf: "required" },
    );
    return allowedGoogleAuthorizationUrl(result.authorization_url);
  }

  async siweChallenge(
    address: string,
    chainId: number,
    intent: Extract<AuthIntent, "signin" | "link">,
  ): Promise<SiweChallenge> {
    return this.#request("/api/v1/auth/siwe/challenge", {
      method: "POST",
      body: { address, chain_id: chainId, intent },
      csrf: intent === "link" ? "required" : "never",
    });
  }

  async verifySiwe(
    challengeId: string,
    message: string,
    signature: string,
    intent: Extract<AuthIntent, "signin" | "link">,
  ): Promise<PublicResult> {
    return this.#request("/api/v1/auth/siwe/verify", {
      method: "POST",
      body: { challenge_id: challengeId, message, signature },
      csrf: intent === "link" ? "required" : "never",
    });
  }

  async identities(): Promise<IdentitySummary[]> {
    const result = await this.#request<{ identities: IdentitySummary[] }>("/api/v1/auth/identities");
    return result.identities;
  }

  removeIdentity(identityId: string): Promise<PublicResult> {
    return this.#request(`/api/v1/auth/identities/${encodeURIComponent(boundedId(identityId))}`, {
      method: "DELETE",
      csrf: "required",
    });
  }

  logout(): Promise<PublicResult> {
    return this.#request("/api/v1/auth/logout", { method: "POST", body: {}, csrf: "required" });
  }

  logoutAll(): Promise<PublicResult> {
    return this.#request("/api/v1/auth/logout-all", {
      method: "POST",
      body: { confirmation: "LOG_OUT_ALL" },
      csrf: "required",
    });
  }

  deleteAccount(): Promise<PublicResult> {
    return this.#request("/api/v1/auth/account", {
      method: "DELETE",
      body: { confirmation: "DELETE_ACCOUNT" },
      csrf: "required",
    });
  }

  nativeAuthorization(id: string, state: string): Promise<NativeAuthorizationSummary> {
    const path = `/api/v1/auth/native/authorizations/${encodeURIComponent(boundedId(id))}`;
    return this.#request(`${path}?state=${encodeURIComponent(boundedState(state))}`);
  }

  approveNativeAuthorization(id: string, state: string): Promise<NativeApprovalResult> {
    return this.#request(
      `/api/v1/auth/native/authorizations/${encodeURIComponent(boundedId(id))}/approve`,
      { method: "POST", body: { state: boundedState(state) }, csrf: "required" },
    );
  }

  billingStatus(): Promise<BillingStatus> {
    return this.#request("/api/v1/billing/status");
  }

  invoice(id: string): Promise<PaymentInvoice> {
    return this.#request(`/api/v1/billing/invoices/${encodeURIComponent(boundedId(id))}`);
  }

  createInvoice(amountUsdCents: number, clientRequestId: string): Promise<PaymentInvoice> {
    if (!Number.isSafeInteger(amountUsdCents) || amountUsdCents <= 0) {
      throw new TypeError("Invoice amount must be a positive integer");
    }
    if (!/^[A-Za-z0-9_.:-]{1,128}$/.test(clientRequestId)) throw new TypeError("Invalid invoice request identifier");
    return this.#request("/api/v1/billing/invoices", {
      method: "POST",
      body: { amount_usd_cents: amountUsdCents, client_request_id: clientRequestId },
      csrf: "required",
    });
  }

  accountFromSession(session: SessionResponse): AccountSummary | null {
    return session.authenticated && session.account ? session.account : null;
  }

  async #request<T>(path: string, options: RequestOptions = {}): Promise<T> {
    const csrf = options.csrf ?? "never";
    if (csrf === "required" && !this.#csrfToken) {
      throw new ApiError(401, "session_required");
    }

    const headers = new Headers({ accept: "application/json" });
    if (options.body !== undefined) headers.set("content-type", "application/json");
    if (csrf !== "never" && this.#csrfToken) headers.set("x-csrf-token", this.#csrfToken);

    const controller = new AbortController();
    const timeoutId = globalThis.setTimeout(() => controller.abort(), DEFAULT_TIMEOUT_MS);
    const abortFromCaller = () => controller.abort();
    if (options.signal?.aborted) controller.abort();
    else options.signal?.addEventListener("abort", abortFromCaller, { once: true });
    let response: Response;
    try {
      response = await fetch(this.#url(path), {
        method: options.method ?? "GET",
        body: options.body === undefined ? undefined : JSON.stringify(options.body),
        credentials: "include",
        headers,
        redirect: "error",
        referrerPolicy: "no-referrer",
        signal: controller.signal,
      });
    } catch (error) {
      if (controller.signal.aborted || (error instanceof DOMException && error.name === "AbortError")) {
        throw new ApiError(408, "request_timeout");
      }
      throw new ApiError(0, "network_error");
    } finally {
      globalThis.clearTimeout(timeoutId);
      options.signal?.removeEventListener("abort", abortFromCaller);
    }

    const payload = await readBoundedJson(response);
    if (!response.ok) {
      const code = isRecord(payload) && typeof payload.code === "string" ? payload.code : undefined;
      throw new ApiError(response.status, safeErrorCode(code));
    }
    return payload as T;
  }

  #url(path: string): URL {
    if (!path.startsWith("/api/") || path.includes("\\")) throw new TypeError("Invalid API path");
    return new URL(path, this.origin);
  }
}

export function normalizeApiOrigin(value: string): string {
  const url = new URL(value);
  const local = url.hostname === "127.0.0.1" || url.hostname === "localhost" || url.hostname === "[::1]";
  if ((url.protocol !== "https:" && !(local && url.protocol === "http:")) || url.username || url.password) {
    throw new TypeError("API origin must use HTTPS (except loopback development)");
  }
  if (url.pathname !== "/" || url.search || url.hash) throw new TypeError("API origin must not include a path");
  return url.origin;
}

export function apiOriginForHost(hostname: string): string {
  return hostname.toLowerCase() === "auth-staging.axiom.stream"
    ? "https://api-staging.axiom.stream"
    : "https://api.axiom.stream";
}

export function safeReturnPath(value: string): string {
  if (!value.startsWith("/") || value.startsWith("//") || value.includes("\\")) return "/";
  try {
    const url = new URL(value, "https://auth.invalid");
    const normalized = url.pathname.replace(/\/+$/, "") || "/";
    const allowed = new Set([
      "/",
      "/signin",
      "/signup",
      "/recover",
      "/reset-password",
      "/verify-email",
      "/account",
      "/native/authorize",
      "/payments/return",
      "/terms",
      "/privacy",
    ]);
    if (url.origin !== "https://auth.invalid" || url.hash || !allowed.has(normalized)) return "/";
    return `${url.pathname}${url.search}`;
  } catch {
    return "/";
  }
}

export function allowedGoogleAuthorizationUrl(value: string): string {
  const url = new URL(value);
  if (
    url.origin !== "https://accounts.google.com" ||
    !url.pathname.startsWith("/o/oauth2/") ||
    url.username ||
    url.password ||
    url.port ||
    url.hash
  ) {
    throw new ApiError(502, "invalid_response");
  }
  return url.toString();
}

async function readBoundedJson(response: Response): Promise<unknown> {
  if (response.status === 204) return {};
  const declaredLength = Number(response.headers.get("content-length") ?? 0);
  if (declaredLength > MAX_RESPONSE_BYTES) throw new ApiError(502, "invalid_response");
  if (!response.body) return {};

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    length += value.byteLength;
    if (length > MAX_RESPONSE_BYTES) {
      await reader.cancel();
      throw new ApiError(502, "invalid_response");
    }
    chunks.push(value);
  }

  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  if (bytes.byteLength === 0) return {};
  try {
    return JSON.parse(new TextDecoder().decode(bytes)) as unknown;
  } catch {
    throw new ApiError(502, "invalid_response");
  }
}

function boundedId(value: string): string {
  if (!/^[A-Za-z0-9_-]{1,200}$/.test(value)) throw new TypeError("Invalid identifier");
  return value;
}

function boundedState(value: string): string {
  if (!/^[A-Za-z0-9._~-]{16,512}$/.test(value)) throw new TypeError("Invalid authorization state");
  return value;
}

function safeErrorCode(value: string | undefined): string {
  return value && /^[a-z0-9_]{1,64}$/.test(value) ? value : "request_failed";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function publicErrorMessage(status: number, code: string): string {
  if (code === "network_error") return "Axiom could not be reached. Check your connection and try again.";
  if (code === "request_timeout") return "The request timed out. Please try again.";
  if (code === "session_required" || status === 401) return "Please sign in again to continue.";
  if (status === 403) return "That action could not be authorized.";
  if (status === 404) return "The requested item was not found or has expired.";
  if (status === 409) return "That identity is already linked to another account.";
  if (status === 429) return "Too many attempts. Wait a moment before trying again.";
  if (status >= 500 || status === 0) return "Axiom is temporarily unavailable. Please try again.";
  return "The request could not be completed. Check the details and try again.";
}

export const authApi = new AuthApiClient();

function emailLoginChallenge(result: EmailLoginChallenge): EmailLoginChallenge {
  if (result.authenticated !== false || !/^[0-9a-f]{32}$/.test(result.challenge_id)
      || !Number.isFinite(Date.parse(result.expires_at))
      || !Number.isInteger(result.resend_after_seconds) || result.resend_after_seconds < 0) {
    throw new ApiError(0, "invalid_response");
  }
  return result;
}
