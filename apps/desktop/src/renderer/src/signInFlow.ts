import type { AccountStatus, BrowserLoginState } from "@axiom/axiom-acp-client";

export type SignInState =
  | { kind: "closed" }
  | { kind: "starting" }
  | { kind: "waiting"; login: BrowserLoginState }
  | { kind: "success" }
  | { kind: "error"; message: string };

export type AccountPresentation =
  | { kind: "starting"; title: "AxiomCLI is starting"; detail: "Your local agent is connecting…" }
  | { kind: "signed-out"; title: "Connect your Axiom account"; detail: "Sign in securely in your browser to access models." }
  | { kind: "invalid"; title: "Reconnect your Axiom account"; detail: string }
  | { kind: "unavailable"; title: "Account status unavailable"; detail: string }
  | { kind: "valid"; title: "Axiom account"; detail: "Connected securely" };

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
  if (account?.state === "valid") {
    return { kind: "valid", title: "Axiom account", detail: "Connected securely" };
  }
  if (account?.state === "invalid") {
    return {
      kind: "invalid",
      title: "Reconnect your Axiom account",
      detail: account.detail?.trim() || "Your saved sign-in is invalid or was revoked.",
    };
  }
  if (account?.state === "unavailable") {
    return {
      kind: "unavailable",
      title: "Account status unavailable",
      detail: account.detail?.trim() || "Axiom could not be reached. Check your connection and try again.",
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
