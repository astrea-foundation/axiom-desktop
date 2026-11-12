export type AccountPresentation =
  | { kind: "starting"; title: string; detail: string; phase?: "checking_sign_in" | "opening_chats" | "switching_account" }
  | { kind: "signed-out"; title: string; detail: string }
  | { kind: "expired"; title: string; detail: string }
  | { kind: "unavailable"; title: string; detail: string; technicalDetails?: string }
  | { kind: "valid"; title: string; detail: string };

