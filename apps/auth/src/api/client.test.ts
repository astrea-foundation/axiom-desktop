import { afterEach, describe, expect, it, vi } from "vitest";

import {
  ApiError,
  AuthApiClient,
  allowedGoogleAuthorizationUrl,
  apiOriginForHost,
  normalizeApiOrigin,
  safeReturnPath,
} from "./client";

afterEach(() => vi.unstubAllGlobals());

describe("normalizeApiOrigin", () => {
  it("accepts HTTPS and loopback development", () => {
    expect(normalizeApiOrigin("https://api.axiom.stream")).toBe("https://api.axiom.stream");
    expect(normalizeApiOrigin("http://127.0.0.1:8080")).toBe("http://127.0.0.1:8080");
  });

  it.each([
    "http://api.axiom.stream",
    "https://user:pass@api.axiom.stream",
    "https://api.axiom.stream/v1",
    "javascript:alert(1)",
  ])("rejects unsafe origin %s", (origin) => {
    expect(() => normalizeApiOrigin(origin)).toThrow(TypeError);
  });
});

describe("apiOriginForHost", () => {
  it("keeps staging assets on the staging API", () => {
    expect(apiOriginForHost("auth-staging.axiom.stream")).toBe("https://api-staging.axiom.stream");
    expect(apiOriginForHost("auth.axiom.stream")).toBe("https://api.axiom.stream");
  });
});

describe("safeReturnPath", () => {
  it("allows explicit auth routes and rejects redirect confusion", () => {
    expect(safeReturnPath("/native/authorize?id=abc")).toBe("/native/authorize?id=abc");
    expect(safeReturnPath("//evil.example/path")).toBe("/");
    expect(safeReturnPath("https://evil.example/path")).toBe("/");
    expect(safeReturnPath("/safe\\evil")).toBe("/");
    expect(safeReturnPath("/chat")).toBe("/");
    expect(safeReturnPath("/account#secret")).toBe("/");
  });
});

describe("allowedGoogleAuthorizationUrl", () => {
  it("allows the exact Google OAuth authorization endpoint", () => {
    expect(allowedGoogleAuthorizationUrl("https://accounts.google.com/o/oauth2/v2/auth?state=opaque")).toBe(
      "https://accounts.google.com/o/oauth2/v2/auth?state=opaque",
    );
  });

  it.each([
    "http://accounts.google.com/o/oauth2/v2/auth",
    "https://evil.accounts.google.com/o/oauth2/v2/auth",
    "https://accounts.google.com.evil.test/o/oauth2/v2/auth",
    "https://accounts.google.com/ServiceLogin",
    "https://accounts.google.com:444/o/oauth2/v2/auth",
    "https://accounts.google.com/o/oauth2/v2/auth#token",
  ])("rejects an unsafe Google navigation target %s", (url) => {
    expect(() => allowedGoogleAuthorizationUrl(url)).toThrow(ApiError);
  });
});

describe("AuthApiClient", () => {
  it("requires the email challenge before accepting a password session", async () => {
    const requests: { url: string; init: RequestInit }[] = [];
    const challenge = {
      authenticated: false, challenge_id: "a".repeat(32),
      expires_at: "2026-09-09T20:00:00Z", resend_after_seconds: 60,
    };
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      requests.push({ url: String(input), init: init ?? {} });
      if (String(input).endsWith("/login") || String(input).endsWith("/resend")) return json(challenge, 202);
      if (String(input).endsWith("/verify")) return json({ authenticated: true, csrf_token: "after-email-proof" });
      return new Response(null, { status: 204 });
    }));
    const client = new AuthApiClient("https://api.axiom.stream");
    expect(await client.passwordLogin("person@example.com", "password")).toEqual(challenge);
    await expect(client.logout()).rejects.toMatchObject({ code: "session_required" });
    await client.resendLoginCode(challenge.challenge_id);
    await client.verifyLoginCode(challenge.challenge_id, "000012");
    await client.logout();
    expect(JSON.parse(String(requests[1]?.init.body))).toEqual({ challenge_id: challenge.challenge_id });
    expect(JSON.parse(String(requests[2]?.init.body))).toEqual({ challenge_id: challenge.challenge_id, code: "000012" });
    expect(requests[2]?.init.credentials).toBe("include");
    expect(new Headers(requests[3]?.init.headers).get("x-csrf-token")).toBe("after-email-proof");
  });

  it("rejects a legacy password-only session response", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => json({ authenticated: true, csrf_token: "legacy" })));
    const client = new AuthApiClient("https://api.axiom.stream");
    await expect(client.passwordLogin("person@example.com", "password")).rejects.toMatchObject({ code: "invalid_response" });
    await expect(client.logout()).rejects.toMatchObject({ code: "session_required" });
  });

  it("sends credentials and the browser-session CSRF token only where required", async () => {
    const requests: RequestInit[] = [];
    const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      requests.push(init ?? {});
      if (requests.length === 1) {
        return json({ authenticated: true, csrf_token: "csrf-value", account: { id: "account_1", linked_methods: [] } });
      }
      return json({ ok: true });
    });
    vi.stubGlobal("fetch", fetchMock);
    const client = new AuthApiClient("https://api.axiom.stream");

    await client.session();
    await client.logout();

    expect(requests[0]?.credentials).toBe("include");
    expect(new Headers(requests[0]?.headers).has("x-csrf-token")).toBe(false);
    expect(new Headers(requests[1]?.headers).get("x-csrf-token")).toBe("csrf-value");
    expect(requests[1]?.redirect).toBe("error");
  });

  it("fails closed when a CSRF-protected request has no loaded session", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const client = new AuthApiClient("https://api.axiom.stream");

    await expect(client.logout()).rejects.toMatchObject({ code: "session_required" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("does not surface an arbitrary backend error body", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => json({ code: "bad!!", detail: "secret provider response" }, 400)));
    const client = new AuthApiClient("https://api.axiom.stream");

    const error = await client.passwordLogin("person@example.com", "not-logged").catch((reason: unknown) => reason);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as Error).message).not.toContain("secret");
    expect((error as ApiError).code).toBe("request_failed");
  });

  it("rejects an oversized response before JSON parsing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", {
      headers: { "content-length": String(65 * 1024), "content-type": "application/json" },
    })));
    const client = new AuthApiClient("https://api.axiom.stream");

    await expect(client.session()).rejects.toMatchObject({ code: "invalid_response" });
  });

  it("constructs only bounded relative Google return URLs", () => {
    const client = new AuthApiClient("https://api.axiom.stream");
    const url = new URL(client.googleStartUrl("//evil.example"));
    expect(url.origin).toBe("https://api.axiom.stream");
    expect(url.pathname).toBe("/api/v1/auth/google/start");
    expect(url.searchParams.get("return_to")).toBe("/");
  });

  it("sends the billing idempotency key with the exact amount", async () => {
    const bodies: unknown[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      bodies.push(init?.body ? JSON.parse(String(init.body)) : null);
      if (bodies.length === 1) return json({ authenticated: true, csrf_token: "csrf-value" });
      return json({ id: "invoice", amount_usd_cents: 2500, state: "pending" });
    }));
    const client = new AuthApiClient("https://api.axiom.stream");
    await client.session();
    await client.createInvoice(2500, "authweb_0123456789abcdef");
    expect(bodies[1]).toEqual({ amount_usd_cents: 2500, client_request_id: "authweb_0123456789abcdef" });
  });

  it("requires CSRF and an exact explicit account-deletion confirmation", async () => {
    const requests: RequestInit[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      requests.push(init ?? {});
      if (requests.length === 1) {
        return json({ authenticated: true, csrf_token: "csrf-value" });
      }
      return new Response(null, { status: 204 });
    }));
    const client = new AuthApiClient("https://api.axiom.stream");
    await client.session();
    await client.deleteAccount();
    expect(requests[1]?.method).toBe("DELETE");
    expect(new Headers(requests[1]?.headers).get("x-csrf-token")).toBe("csrf-value");
    expect(JSON.parse(String(requests[1]?.body))).toEqual({ confirmation: "DELETE_ACCOUNT" });
  });

  it("uses the backend password-reset field and unwraps identity lists", async () => {
    const bodies: unknown[] = [];
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      bodies.push(init?.body ? JSON.parse(String(init.body)) : null);
      if (String(input).endsWith("/identities")) {
        return json({ identities: [{ id: "identity_1", kind: "passkey", label: null, summary: "Passkey", created_at: "now", last_used_at: null }] });
      }
      return new Response(null, { status: 204 });
    }));
    const client = new AuthApiClient("https://api.axiom.stream");
    await client.resetPassword("abcdefghijklmnop_0123456789abcdef", "a long replacement password");
    const identities = await client.identities();
    expect(bodies[0]).toEqual({ token: "abcdefghijklmnop_0123456789abcdef", new_password: "a long replacement password" });
    expect(identities).toHaveLength(1);
  });

  it("creates password credentials only at email-proof completion", async () => {
    const requests: RequestInit[] = [];
    vi.stubGlobal("fetch", vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
      requests.push(init ?? {});
      if (requests.length === 1) {
        return json({ authenticated: true, csrf_token: "csrf-value" });
      }
      return new Response(null, { status: 204 });
    }));
    const client = new AuthApiClient("https://api.axiom.stream");
    await client.session();
    await client.passwordSignup({
      email: "person@example.com",
      display_name: "Person",
      intent: "link",
    });
    await client.verifyEmail(
      "abcdefghijklmnop_0123456789abcdef",
      "a unique password-manager passphrase",
    );

    expect(JSON.parse(String(requests[1]?.body))).toEqual({
      email: "person@example.com",
      display_name: "Person",
    });
    expect(JSON.parse(String(requests[2]?.body))).toEqual({
      token: "abcdefghijklmnop_0123456789abcdef",
      new_password: "a unique password-manager passphrase",
    });
    expect(new Headers(requests[1]?.headers).get("x-csrf-token")).toBe("csrf-value");
    expect(new Headers(requests[2]?.headers).get("x-csrf-token")).toBe("csrf-value");
  });
});

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}
