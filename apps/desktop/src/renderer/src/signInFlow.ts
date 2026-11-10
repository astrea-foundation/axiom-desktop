import type { AccountStatus, NativeLoginState } from "@axiom/axiom-acp-client";

export type SignInState =
  | { kind: "closed" }
  | { kind: "starting" }
  | { kind: "waiting"; login: NativeLoginState }
  | { kind: "success" }
  | { kind: "error"; message: string };

export type { AccountPresentation } from "@axiom/chat-core/account";
import type { AccountPresentation } from "@axiom/chat-core/account";

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

export function signInErrorPresentation(message: string): {
  title: string;
  message: string;
  technicalDetails?: string;
} {
  // This cause identifies invalid TLS data, not the component that sent it.
  // Only a failed start establishes that browser authorization has not begun.
  if (message.includes("could not start login:")
    && message.includes("received corrupt message of type InvalidContentType")) {
    return {
      title: "Couldn’t connect securely",
      message: "Axiom received an invalid response while establishing a secure connection, so browser sign-in could not start. Check any VPN, proxy, or network filter, or try another network such as a phone hotspot.",
      technicalDetails: message,
    };
  }
  return { title: "Sign-in didn’t finish", message };
}
