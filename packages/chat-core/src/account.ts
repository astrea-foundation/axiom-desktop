export type AccountPreparationPhase = "checking_sign_in" | "opening_chats" | "decrypting_chats" | "switching_account";

export type AccountPresentation =
  | { kind: "starting"; title: string; detail: string; phase?: AccountPreparationPhase }
  | { kind: "signed-out"; title: string; detail: string }
  | { kind: "expired"; title: string; detail: string }
  | { kind: "unavailable"; title: string; detail: string; phase?: AccountPreparationPhase; technicalDetails?: string }
  | { kind: "valid"; title: string; detail: string };

export function isChatPreparation(account: AccountPresentation): account is
  Extract<AccountPresentation, { kind: "starting" }>
  | (Extract<AccountPresentation, { kind: "unavailable" }> & { phase: AccountPreparationPhase }) {
  return account.kind === "starting" || (account.kind === "unavailable" && !!account.phase);
}

