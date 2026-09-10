export type AuthRoute =
  | "signin"
  | "signup"
  | "recover"
  | "reset"
  | "verify"
  | "account"
  | "native"
  | "payment"
  | "terms"
  | "privacy"
  | "not-found";

const ROUTES: Record<string, AuthRoute> = {
  "/": "signin",
  "/signin": "signin",
  "/signup": "signup",
  "/recover": "recover",
  "/reset-password": "reset",
  "/verify-email": "verify",
  "/account": "account",
  "/native/authorize": "native",
  "/payments/return": "payment",
  "/terms": "terms",
  "/privacy": "privacy",
};

export function routeForPath(pathname: string): AuthRoute {
  const normalized = pathname.replace(/\/+$/, "") || "/";
  return ROUTES[normalized] ?? "not-found";
}

export function tokenFromFragment(hash: string): string | null {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  const token = params.get("token");
  if (!token || !/^[A-Za-z0-9._~-]{16,2048}$/.test(token)) return null;
  return token;
}

interface FragmentTokenSource {
  location: { hash: string };
  addEventListener(type: "hashchange", listener: () => void): void;
  removeEventListener(type: "hashchange", listener: () => void): void;
}

/** Email clients can reuse the current tab without reloading the document. */
export function watchTokenFragment(source: FragmentTokenSource, onToken: (token: string | null) => void): () => void {
  const changed = () => onToken(tokenFromFragment(source.location.hash));
  source.addEventListener("hashchange", changed);
  return () => source.removeEventListener("hashchange", changed);
}

export function nativeLoopbackCallback(
  value: string | undefined,
  authorizationId: string,
  authorizationState: string,
): string | null {
  if (
    !value ||
    value.length > 2_048 ||
    !/^[0-9a-f]{32}$/.test(authorizationId) ||
    !/^[A-Za-z0-9_-]{32,128}$/.test(authorizationState)
  ) {
    return null;
  }
  try {
    const url = new URL(value);
    const port = Number(url.port);
    if (
      url.protocol !== "http:" ||
      (url.hostname !== "127.0.0.1" && url.hostname !== "[::1]") ||
      !Number.isInteger(port) ||
      port < 1_024 ||
      port > 65_535 ||
      url.username ||
      url.password ||
      url.pathname !== "/callback" ||
      url.search ||
      url.hash
    ) {
      return null;
    }
    url.searchParams.set("authorization_id", authorizationId);
    url.searchParams.set("state", authorizationState);
    return url.toString();
  } catch {
    return null;
  }
}
