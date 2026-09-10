import { describe, expect, it } from "vitest";

import { nativeLoopbackCallback, routeForPath, tokenFromFragment, watchTokenFragment } from "./navigation";

describe("routeForPath", () => {
  it("routes only the explicit authentication surface", () => {
    expect(routeForPath("/native/authorize/")).toBe("native");
    expect(routeForPath("/chat")).toBe("not-found");
    expect(routeForPath("/api/v1/auth/session")).toBe("not-found");
  });
});

describe("tokenFromFragment", () => {
  it("accepts a bounded fragment token and rejects query-like input", () => {
    expect(tokenFromFragment("#token=abcdefghijklmnop_ABC-123")).toBe("abcdefghijklmnop_ABC-123");
    expect(tokenFromFragment("#token=short")).toBeNull();
    expect(tokenFromFragment("#token=abc%26leak%3Dyesabcdefghijklmnop")).toBeNull();
  });
});

describe("watchTokenFragment", () => {
  it("replaces a same-tab email capability, fails closed on invalid input, and unsubscribes", () => {
    const source = Object.assign(new EventTarget(), { location: { hash: "" } });
    const tokens: Array<string | null> = [];
    const stop = watchTokenFragment(source, (token) => tokens.push(token));
    source.location.hash = "#token=abcdefghijklmnop_NEW-123";
    source.dispatchEvent(new Event("hashchange"));
    source.location.hash = "#token=invalid";
    source.dispatchEvent(new Event("hashchange"));
    expect(tokens).toEqual(["abcdefghijklmnop_NEW-123", null]);
    stop();
    source.location.hash = "#token=abcdefghijklmnop_LATER-456";
    source.dispatchEvent(new Event("hashchange"));
    expect(tokens).toHaveLength(2);
  });
});

describe("nativeLoopbackCallback", () => {
  const authorizationId = "0123456789abcdef0123456789abcdef";
  const state = "abcdefghijklmnopqrstuvwxyzABCDEFG_0123456789-abcdefghijklmnopqrstuvwxyz";

  function callback(value: string | undefined) {
    return nativeLoopbackCallback(value, authorizationId, state);
  }

  it("allows only numeric IPv4 or bracketed IPv6 loopback with an explicit port", () => {
    const query = `?authorization_id=${authorizationId}&state=${state}`;
    expect(callback("http://127.0.0.1:49152/callback")).toBe(`http://127.0.0.1:49152/callback${query}`);
    expect(callback("http://localhost:49152/callback")).toBeNull();
    expect(callback("http://[::1]:49152/callback")).toBe(`http://[::1]:49152/callback${query}`);
    expect(callback("https://127.0.0.1:49152/callback")).toBeNull();
    expect(callback("http://127.0.0.1/callback")).toBeNull();
    expect(callback("http://127.0.0.1:80/callback")).toBeNull();
    expect(callback("http://127.0.0.1:49152/other")).toBeNull();
    expect(callback("http://127.0.0.1:49152/callback?code=done")).toBeNull();
    expect(callback("http://127.0.0.1:49152/callback#secret")).toBeNull();
    expect(nativeLoopbackCallback("http://127.0.0.1:49152/callback", "bad", state)).toBeNull();
    expect(nativeLoopbackCallback("http://127.0.0.1:49152/callback", authorizationId, "bad")).toBeNull();
  });
});
