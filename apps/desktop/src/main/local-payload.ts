import type { ClientState, LocalPayloadRequest } from "@axiom/axiom-acp-client";
import { localEnvelope } from "../shared/local-payload";

export function requireLocalPayloadAccount(accountId: string, state: ClientState): void {
  if (!state.connected || state.account?.state !== "valid" || state.account.account?.id !== accountId) {
    throw new Error("The account changed. Local data was not accessed.");
  }
}

export function localPayloadRequest(value: unknown): LocalPayloadRequest {
  if (!value || typeof value !== "object") throw new Error("Invalid local storage request.");
  const request = value as LocalPayloadRequest;
  if (typeof request.accountId !== "string" || !request.accountId || request.accountId.length > 512
    || typeof request.id !== "string" || !request.id || request.id.length > 2048 || /[\x00-\x1f\x7f]/.test(request.id)
    || !request.action || typeof request.action !== "object") throw new Error("Invalid local storage identity.");
  switch (request.action.kind) {
    case "get": case "remove": return { accountId: request.accountId, id: request.id, action: { kind: request.action.kind } };
    case "put": return { accountId: request.accountId, id: request.id, action: { kind: "put", value: localEnvelope(request.action.value) } };
    case "import_legacy":
      if (request.action.preview !== undefined && typeof request.action.preview !== "boolean") throw new Error("Invalid legacy import mode.");
      return { accountId: request.accountId, id: request.id, action: { kind: "import_legacy", value: localEnvelope(request.action.value, true), preview: request.action.preview } };
    default: throw new Error("Unsupported local storage operation.");
  }
}
