import { describe, expect, it } from "vitest";

import { allowedShieldzPayUrl } from "./payments";

describe("allowedShieldzPayUrl", () => {
  it("allows only the exact v1 Shieldz origin and payment path", () => {
    expect(allowedShieldzPayUrl("https://shieldz.cash/pay/invoice_123")).toBe(
      "https://shieldz.cash/pay/invoice_123",
    );
  });

  it.each([
    "http://shieldz.cash/pay/invoice_123",
    "https://checkout.shieldz.cash/pay/invoice_123",
    "https://shieldz.cash:444/pay/invoice_123",
    "https://user:pass@shieldz.cash/pay/invoice_123",
    "https://shieldz.cash/checkout/invoice_123",
    "https://shieldz.cash/pay/",
    "https://shieldz.cash/pay/invoice_123/extra",
    "https://shieldz.cash/pay/invoice.123",
    "https://shieldz.cash/pay/%69nvoice_123",
    "https://shieldz.cash/pay/invoice_123?token=opaque",
    "https://shieldz.cash/pay/invoice_123#secret",
    "https://shieldz.cash.evil.example/pay/invoice_123",
  ])("rejects unsafe payment target %s", (url) => {
    expect(() => allowedShieldzPayUrl(url)).toThrow(TypeError);
  });
});
