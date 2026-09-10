import { describe, expect, it } from "vitest";

import { authWorker, contentSecurityPolicy, type AuthWorkerEnv } from "./worker";

describe("auth Worker", () => {
  it("serves application routes through no-store HTML with strict headers", async () => {
    const response = await authWorker.fetch(new Request("https://auth.axiom.stream/native/authorize?authorization_id=x"), env());
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("<main>auth</main>");
    expect(response.headers.get("cache-control")).toBe("no-store");
    expect(response.headers.get("x-frame-options")).toBe("DENY");
    expect(response.headers.get("cross-origin-opener-policy")).toBe("same-origin");
    expect(response.headers.get("permissions-policy")).toContain("publickey-credentials-get=(self)");
    expect(response.headers.get("content-security-policy")).toContain("connect-src 'self' https://api.axiom.stream");
  });

  it("caches only fingerprinted Vite assets as immutable", async () => {
    const immutable = await authWorker.fetch(new Request("https://auth.axiom.stream/assets/app-Abcdef12.js"), env());
    const unhashed = await authWorker.fetch(new Request("https://auth.axiom.stream/favicon.svg"), env());
    expect(immutable.headers.get("cache-control")).toBe("public, max-age=31536000, immutable");
    expect(unhashed.headers.get("cache-control")).toBe("no-cache");
  });

  it("rejects wrong hosts, mutations, and encoded separator paths", async () => {
    expect((await authWorker.fetch(new Request("https://evil.example/"), env())).status).toBe(421);
    expect((await authWorker.fetch(new Request("https://auth.axiom.stream/", { method: "POST" }), env())).status).toBe(405);
    expect((await authWorker.fetch(new Request("https://auth.axiom.stream/a%5cb"), env())).status).toBe(404);
  });

  it("serves the SPA shell only for the explicit hosted-auth route set", async () => {
    expect((await authWorker.fetch(new Request("https://auth.axiom.stream/account/"), env())).status).toBe(200);
    expect((await authWorker.fetch(new Request("https://auth.axiom.stream/chat"), env())).status).toBe(404);
    expect((await authWorker.fetch(new Request("https://auth.axiom.stream/api/v1/auth/session"), env())).status).toBe(404);
  });

  it("renders the branded 404 for document navigation without swallowing API or asset errors", async () => {
    const request = (path: string) => new Request(`https://auth.axiom.stream${path}`, { headers: { accept: "text/html" } });
    const missing = await authWorker.fetch(request("/not-a-page"), env());
    expect(missing.status).toBe(404);
    expect(missing.headers.get("content-type")).toContain("text/html");
    expect(missing.headers.get("cache-control")).toBe("no-store");
    expect(await missing.text()).toBe("<main>auth</main>");
    expect(await (await authWorker.fetch(request("/api/v1/missing"), env())).text()).toBe("Not Found");
    expect(await (await authWorker.fetch(request("/assets/missing.js"), env())).text()).toBe("Not Found");
  });

  it("returns an empty body for HEAD", async () => {
    const response = await authWorker.fetch(new Request("https://auth.axiom.stream/", { method: "HEAD" }), env());
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("");
  });

  it("does not forward account cookies into static assets or emit asset cookies", async () => {
    let assetCookie: string | null = "not-called";
    const testEnv = env();
    testEnv.ASSETS = {
      async fetch(request) {
        assetCookie = request.headers.get("cookie");
        return new Response("<main>auth</main>", {
          headers: { "content-type": "text/html", "set-cookie": "unsafe=1" },
        });
      },
    };
    const response = await authWorker.fetch(
      new Request("https://auth.axiom.stream/", { headers: { cookie: "session=secret" } }),
      testEnv,
    );
    expect(assetCookie).toBeNull();
    expect(response.headers.has("set-cookie")).toBe(false);
  });
});

describe("contentSecurityPolicy", () => {
  it("fails closed for an unsafe API origin", () => {
    expect(contentSecurityPolicy("http://api.axiom.stream")).toContain("connect-src 'self';");
    expect(contentSecurityPolicy("https://api.axiom.stream/path")).toContain("connect-src 'self';");
    expect(contentSecurityPolicy("https://api.axiom.stream?unsafe=yes")).toContain("connect-src 'self';");
  });
});

function env(): AuthWorkerEnv {
  return {
    API_ORIGIN: "https://api.axiom.stream",
    AUTH_HOST: "auth.axiom.stream",
    ENVIRONMENT: "production",
    ASSETS: {
      async fetch(request: Request) {
        const path = new URL(request.url).pathname;
        if (path === "/index.html") {
          return new Response("<main>auth</main>", { headers: { "content-type": "text/html; charset=utf-8" } });
        }
        if (path === "/assets/app-Abcdef12.js") {
          return new Response("export{}", { headers: { "content-type": "text/javascript" } });
        }
        if (path === "/favicon.svg") {
          return new Response("<svg/>", { headers: { "content-type": "image/svg+xml" } });
        }
        return new Response("missing", { status: 404 });
      },
    },
  };
}
