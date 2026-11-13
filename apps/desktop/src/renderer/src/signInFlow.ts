import type { AccountStatus, LoginMethod, NativeLoginState } from "@axiom/axiom-acp-client";

export const LOGIN_CHOICES: ReadonlyArray<{ method: LoginMethod; label: string; detail: string }> = [
  { method: "passkey", label: "Passkey", detail: "Use Touch ID, Windows Hello, or a security key" },
  { method: "google", label: "Google", detail: "Continue with your Google account" },
  { method: "password", label: "Email and password", detail: "Use your verified email address" },
  { method: "ethereum_wallet", label: "Ethereum wallet", detail: "Sign a standard SIWE message" },
];

export function loginMethodLabel(method: LoginMethod): string {
  return LOGIN_CHOICES.find((choice) => choice.method === method)?.label ?? "Axiom";
}

export type SignInState =
  | { kind: "closed" }
  | { kind: "choose" }
  | { kind: "starting"; method: LoginMethod }
  | { kind: "waiting"; login: NativeLoginState; method: LoginMethod }
  | { kind: "success" }
  | { kind: "error"; message: string };

export type AccountPresentation =
  | { kind: "starting"; title: string; detail: string }
  | { kind: "signed-out"; title: string; detail: string }
  | { kind: "expired"; title: string; detail: string }
  | { kind: "unavailable"; title: string; detail: string }
  | { kind: "valid"; title: string; detail: string };

export function accountPresentation(
  connected: boolean,
  desktopReady: boolean,
  account: AccountStatus | null,
): AccountPresentation {
  if (!connected || !desktopReady) {
    return {
      kind: "starting",
      title: "AxiomCLI is starting",
      detail: "Your local agent is connecting…",
    };
  }
  if (account?.state === "valid" && account.account) {
    const title = account.account.displayName?.trim()
      || account.account.verifiedEmail?.trim()
      || "Axiom account";
    const detail = account.account.displayName?.trim() && account.account.verifiedEmail?.trim()
      ? account.account.verifiedEmail.trim()
      : "Connected securely";
    return { kind: "valid", title, detail };
  }
  if (account?.state === "expired") {
    return {
      kind: "expired",
      title: "Reconnect your Axiom account",
      detail: account.detail?.trim() || "Your saved session expired or was revoked.",
    };
  }
  if (account?.state === "unavailable") {
    return {
      kind: "unavailable",
      title: "Account status unavailable",
      detail: account.detail?.trim() || "Axiom could not be reached. Check your connection and try again.",
    };
  }
  if (account?.state === "starting") {
    return {
      kind: "starting",
      title: "Checking your Axiom account",
      detail: account.detail?.trim() || "Your local agent is restoring its secure session…",
    };
  }
  return {
    kind: "signed-out",
    title: "Connect your Axiom account",
    detail: "Sign in securely in your browser to access models.",
  };
}

export function desktopErrorMessage(error: unknown, fallback: string): string {
  const raw = error instanceof Error ? error.message : String(error || "");
  const message = raw
    .replace(/^Error invoking remote method ['\"]?agent:[^'\"]+['\"]?:\s*/i, "")
    .replace(/^(?:AcpError|Error):\s*/i, "")
    .trim();
  return message || fallback;
}

export function signInErrorMessage(error: unknown): string {
  return desktopErrorMessage(
    error,
    "Browser sign-in could not be completed. Please try again.",
  );
}
