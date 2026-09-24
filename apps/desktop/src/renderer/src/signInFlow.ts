import type { AccountStatus, NativeLoginState } from "@axiom/axiom-acp-client";

export type SignInState =
  | { kind: "closed" }
  | { kind: "starting" }
  | { kind: "waiting"; login: NativeLoginState }
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
      title: "Starting Axiom…",
      detail: "",
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
      title: "Checking your account…",
      detail: "",
    };
  }
  return {
    kind: "signed-out",
    title: "Sign in",
    detail: "Continue in your browser.",
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
