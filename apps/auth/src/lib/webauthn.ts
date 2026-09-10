import type { PasskeyCreationOptions, PasskeyRequestOptions } from "../api/types";
import { decodeBase64Url, encodeBase64Url } from "./base64url";

export function passkeysAvailable(): boolean {
  return typeof window !== "undefined" && "PublicKeyCredential" in window && "credentials" in navigator;
}

export function toCreationOptions(input: PasskeyCreationOptions["public_key"]): PublicKeyCredentialCreationOptions {
  return {
    ...input,
    challenge: decodeBase64Url(input.challenge),
    user: { ...input.user, id: decodeBase64Url(input.user.id) },
    excludeCredentials: input.excludeCredentials?.map((descriptor) => ({
      ...descriptor,
      id: decodeBase64Url(descriptor.id),
    })),
  };
}

export function toRequestOptions(input: PasskeyRequestOptions["public_key"]): PublicKeyCredentialRequestOptions {
  return {
    ...input,
    challenge: decodeBase64Url(input.challenge),
    allowCredentials: input.allowCredentials?.map((descriptor) => ({
      ...descriptor,
      id: decodeBase64Url(descriptor.id),
    })),
  };
}

export function serializeCredential(credential: PublicKeyCredential): Record<string, unknown> {
  const response = credential.response;
  const base = {
    id: credential.id,
    rawId: encodeBase64Url(credential.rawId),
    type: credential.type,
    authenticatorAttachment: credential.authenticatorAttachment,
    clientExtensionResults: credential.getClientExtensionResults(),
  };

  if (response instanceof AuthenticatorAttestationResponse) {
    return {
      ...base,
      response: {
        attestationObject: encodeBase64Url(response.attestationObject),
        clientDataJSON: encodeBase64Url(response.clientDataJSON),
        transports: typeof response.getTransports === "function" ? response.getTransports() : [],
      },
    };
  }
  if (response instanceof AuthenticatorAssertionResponse) {
    return {
      ...base,
      response: {
        authenticatorData: encodeBase64Url(response.authenticatorData),
        clientDataJSON: encodeBase64Url(response.clientDataJSON),
        signature: encodeBase64Url(response.signature),
        userHandle: response.userHandle ? encodeBase64Url(response.userHandle) : null,
      },
    };
  }
  throw new TypeError("Unsupported passkey response");
}

export async function authenticateWithPasskey(options: PasskeyRequestOptions): Promise<Record<string, unknown>> {
  ensurePasskeysAvailable();
  const credential = await navigator.credentials.get({ publicKey: toRequestOptions(options.public_key) });
  if (!(credential instanceof PublicKeyCredential)) throw new DOMException("No passkey selected", "NotAllowedError");
  return serializeCredential(credential);
}

export async function registerPasskey(options: PasskeyCreationOptions): Promise<Record<string, unknown>> {
  ensurePasskeysAvailable();
  const credential = await navigator.credentials.create({ publicKey: toCreationOptions(options.public_key) });
  if (!(credential instanceof PublicKeyCredential)) throw new DOMException("Passkey creation cancelled", "NotAllowedError");
  return serializeCredential(credential);
}

function ensurePasskeysAvailable(): void {
  if (!passkeysAvailable()) throw new Error("Passkeys are not available in this browser.");
}
