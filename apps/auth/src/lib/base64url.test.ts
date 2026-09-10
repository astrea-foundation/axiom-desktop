import { describe, expect, it } from "vitest";

import { decodeBase64Url, encodeBase64Url } from "./base64url";

describe("base64url", () => {
  it("round trips WebAuthn bytes without padding", () => {
    const source = Uint8Array.from([0, 1, 2, 253, 254, 255]);
    const encoded = encodeBase64Url(source);
    expect(encoded).toBe("AAEC_f7_");
    expect(Array.from(new Uint8Array(decodeBase64Url(encoded)))).toEqual(Array.from(source));
  });

  it("rejects standard-base64 and punctuation", () => {
    expect(() => decodeBase64Url("a+b/")).toThrow(TypeError);
    expect(() => decodeBase64Url("abc=")).toThrow(TypeError);
  });
});
