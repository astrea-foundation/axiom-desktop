export type AccountPresentation =
  | { kind: "starting"; title: string; detail: string }
  | { kind: "signed-out"; title: string; detail: string }
  | { kind: "expired"; title: string; detail: string }
  | { kind: "unavailable"; title: string; detail: string }
  | { kind: "valid"; title: string; detail: string };

