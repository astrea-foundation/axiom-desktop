interface AssetBinding {
  fetch(request: Request): Promise<Response>;
}

export interface AuthWorkerEnv {
  ASSETS: AssetBinding;
  API_ORIGIN: string;
  AUTH_HOST: string;
  ENVIRONMENT: "development" | "staging" | "production";
}

const IMMUTABLE_ASSET = /^\/assets\/[^/]+-[A-Za-z0-9_-]{8,}\.(?:css|js|woff2?)$/;
const METHODS = new Set(["GET", "HEAD"]);
const APPLICATION_ROUTES = new Set([
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

export const authWorker = {
  async fetch(request: Request, env: AuthWorkerEnv): Promise<Response> {
    const url = new URL(request.url);
    if (!hostAllowed(url.hostname, env)) {
      return textResponse("Misdirected Request", 421, env);
    }

    if (!METHODS.has(request.method)) {
      return textResponse("Method Not Allowed", 405, env, { allow: "GET, HEAD" });
    }

    if (isUnsafePath(url)) {
      return textResponse("Not Found", 404, env);
    }

    const assetRequest = assetRequestFor(url, request.method);
    let asset = await env.ASSETS.fetch(assetRequest);
    let notFoundPage = false;

    if (asset.status === 404 && isApplicationRoute(url.pathname)) {
      const indexUrl = new URL(url);
      indexUrl.pathname = "/index.html";
      indexUrl.search = "";
      asset = await env.ASSETS.fetch(assetRequestFor(indexUrl, request.method));
    }

    if (asset.status === 404 && request.headers.get("accept")?.includes("text/html")
      && !url.pathname.startsWith("/api/") && !url.pathname.startsWith("/assets/")
      && !url.pathname.split("/").pop()?.includes(".")) {
      const indexUrl = new URL(url);
      indexUrl.pathname = "/index.html";
      indexUrl.search = "";
      asset = await env.ASSETS.fetch(assetRequestFor(indexUrl, request.method));
      notFoundPage = true;
    }
    if (asset.status === 404) return textResponse("Not Found", 404, env);

    const headers = new Headers(asset.headers);
    headers.set("cache-control", cachePolicy(url.pathname, headers.get("content-type")));
    applySecurityHeaders(headers, env);

    return new Response(request.method === "HEAD" ? null : asset.body, {
      status: notFoundPage ? 404 : asset.status,
      statusText: asset.statusText,
      headers,
    });
  },
};

export default authWorker;

export function applySecurityHeaders(headers: Headers, env: AuthWorkerEnv): void {
  headers.set("content-security-policy", contentSecurityPolicy(env.API_ORIGIN));
  headers.set("cross-origin-opener-policy", "same-origin");
  headers.set("cross-origin-resource-policy", "same-origin");
  headers.set("permissions-policy", permissionsPolicy());
  headers.set("referrer-policy", "no-referrer");
  headers.set("strict-transport-security", "max-age=63072000; includeSubDomains; preload");
  headers.set("x-content-type-options", "nosniff");
  headers.set("x-frame-options", "DENY");
  headers.set("x-permitted-cross-domain-policies", "none");
  headers.delete("server");
  headers.delete("set-cookie");
  headers.delete("set-cookie2");
  headers.delete("x-powered-by");
}

export function contentSecurityPolicy(apiOrigin: string): string {
  const connectOrigin = safeHttpsOrigin(apiOrigin);
  return [
    "default-src 'none'",
    "base-uri 'none'",
    `connect-src 'self'${connectOrigin ? ` ${connectOrigin}` : ""}`,
    "font-src 'self'",
    "form-action 'self'",
    "frame-ancestors 'none'",
    "frame-src 'none'",
    "img-src 'self' data:",
    "manifest-src 'self'",
    "media-src 'none'",
    "object-src 'none'",
    "script-src 'self'",
    "style-src 'self'",
    "worker-src 'none'",
    "upgrade-insecure-requests",
  ].join("; ");
}

function permissionsPolicy(): string {
  return [
    "accelerometer=()",
    "ambient-light-sensor=()",
    "autoplay=()",
    "browsing-topics=()",
    "camera=()",
    "display-capture=()",
    "geolocation=()",
    "gyroscope=()",
    "magnetometer=()",
    "microphone=()",
    "payment=()",
    "publickey-credentials-create=(self)",
    "publickey-credentials-get=(self)",
    "serial=()",
    "usb=()",
  ].join(", ");
}

function safeHttpsOrigin(value: string): string | null {
  try {
    const url = new URL(value);
    if (
      url.protocol !== "https:" ||
      url.username ||
      url.password ||
      url.port ||
      url.pathname !== "/" ||
      url.search ||
      url.hash
    ) {
      return null;
    }
    return url.origin;
  } catch {
    return null;
  }
}

function assetRequestFor(url: URL, method: string): Request {
  return new Request(url, { method, headers: { accept: "*/*" } });
}

function hostAllowed(hostname: string, env: AuthWorkerEnv): boolean {
  if (hostname.toLowerCase() === env.AUTH_HOST.toLowerCase()) return true;
  return (
    env.ENVIRONMENT === "development" &&
    (hostname === "127.0.0.1" || hostname === "localhost" || hostname === "[::1]")
  );
}

function isUnsafePath(url: URL): boolean {
  const raw = url.pathname.toLowerCase();
  return (
    raw.includes("\\") ||
    raw.includes("%2f") ||
    raw.includes("%5c") ||
    raw.split("/").some((part) => part === "." || part === ".." || part.startsWith("."))
  );
}

function isApplicationRoute(pathname: string): boolean {
  const normalized = pathname.replace(/\/+$/, "") || "/";
  return APPLICATION_ROUTES.has(normalized);
}

function cachePolicy(pathname: string, contentType: string | null): string {
  if (contentType?.toLowerCase().includes("text/html") || pathname === "/index.html") {
    return "no-store";
  }
  if (IMMUTABLE_ASSET.test(pathname)) return "public, max-age=31536000, immutable";
  return "no-cache";
}

function textResponse(
  body: string,
  status: number,
  env: AuthWorkerEnv,
  extra: Record<string, string> = {},
): Response {
  const headers = new Headers({
    "cache-control": "no-store",
    "content-type": "text/plain; charset=utf-8",
    ...extra,
  });
  applySecurityHeaders(headers, env);
  return new Response(body, { status, headers });
}
