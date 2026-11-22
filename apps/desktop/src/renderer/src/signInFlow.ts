import type { ChatLocale } from "@axiom/chat-ui";
import { desktopMessages } from "./lib/desktopMessages";
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
  locale: ChatLocale = "en",
): AccountPresentation {
  const copy = desktopMessages[locale];
  if (!connected || !desktopReady) {
    return {
      kind: "starting",
      title: copy.starting,
      detail: copy.connecting,
    };
  }
  if (account?.state === "valid" && account.account) {
    const title = account.account.displayName?.trim()
      || account.account.verifiedEmail?.trim()
      || copy.account;
    const detail = account.account.displayName?.trim() && account.account.verifiedEmail?.trim()
      ? account.account.verifiedEmail.trim()
      : copy.connected;
    return { kind: "valid", title, detail };
  }
  if (account?.state === "expired") {
    return {
      kind: "expired",
      title: copy.reconnect,
      detail: account.detail?.trim() || copy.expired,
    };
  }
  if (account?.state === "unavailable") {
    const storage = account.preparationPhase === "opening_chats" || account.preparationPhase === "decrypting_chats";
    const switching = account.preparationPhase === "switching_account";
    return {
      kind: "unavailable",
      phase: account.preparationPhase ?? undefined,
      title: storage ? copy.chatsUnavailable : switching ? copy.accountUnavailable : copy.signInUnavailable,
      detail: storage
        ? copy.storageError
        : switching ? copy.switchError
          : copy.signInError,
      technicalDetails: account.detail?.trim() || undefined,
    };
  }
  if (!account || account.state === "starting") {
    if (account?.preparationPhase === "switching_account") return {
      kind: "starting", phase: "switching_account",
      title: copy.updatingAccount,
      detail: copy.finishingSwitch,
    };
    if (account?.preparationPhase === "decrypting_chats") return {
      kind: "starting", phase: "decrypting_chats",
      title: copy.decrypting,
      detail: copy.openingSaved,
    };
    const storage = account?.preparationPhase === "opening_chats";
    return {
      kind: "starting",
      phase: storage ? "opening_chats" : "checking_sign_in",
      title: storage ? copy.unlocking : copy.checkingSignIn,
      detail: storage
        ? copy.gettingKey
        : copy.restoringSession,
    };
  }
  return {
    kind: "signed-out",
    title: copy.signIn,
    detail: copy.continueInBrowser,
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
