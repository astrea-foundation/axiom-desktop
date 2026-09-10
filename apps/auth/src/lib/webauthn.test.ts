import { describe, expect, it } from "vitest";

import { toCreationOptions, toRequestOptions } from "./webauthn";

describe("WebAuthn option conversion", () => {
  it("decodes registration challenge, user handle, and excluded credential", () => {
    const options = toCreationOptions({
      challenge: "AQID",
      rp: { id: "auth.axiom.stream", name: "Axiom" },
      user: { id: "BAUG", name: "opaque", displayName: "Axiom account" },
      pubKeyCredParams: [{ type: "public-key", alg: -7 }],
      excludeCredentials: [{ id: "BwgJ", type: "public-key", transports: ["internal"] }],
    });
    expect(bytes(options.challenge)).toEqual([1, 2, 3]);
    expect(bytes(options.user.id)).toEqual([4, 5, 6]);
    expect(bytes(options.excludeCredentials?.[0]?.id as BufferSource)).toEqual([7, 8, 9]);
  });

  it("decodes authentication allow-list credentials", () => {
    const options = toRequestOptions({
      challenge: "AQID",
      rpId: "auth.axiom.stream",
      allowCredentials: [{ id: "BAUG", type: "public-key" }],
      userVerification: "required",
    });
    expect(bytes(options.challenge)).toEqual([1, 2, 3]);
    expect(bytes(options.allowCredentials?.[0]?.id as BufferSource)).toEqual([4, 5, 6]);
    expect(options.rpId).toBe("auth.axiom.stream");
  });
});

function bytes(value: BufferSource): number[] {
  if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
  return Array.from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength));
}
