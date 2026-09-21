import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { BalanceScreen } from "../src/renderer/src/components/BalanceScreen.js";

function render(signedIn = true) {
  return renderToStaticMarkup(createElement(BalanceScreen, {
    connected: true,
    account: signedIn
      ? { revision: 1, state: "valid", account: { id: "account-a", linkedMethods: ["password"] } }
      : { revision: 2, state: "signed_out" },
    billing: {
      revision: 1,
      ledgerSequence: 0, currency: "microUSD",
      postedMicrousd: 3_000_000, availableMicrousd: 3_000_000, trialMicrousd: 1_000_000, paidMicrousd: 2_000_000,
    },
    onClose: () => {},
    onRefreshBilling: async () => {},
    onLogin: () => {},
  }));
}

test("balance distinguishes revocable trial credit from other credit and ZEC", () => {
  const html = render();
  assert.match(html, /Credit breakdown/);
  assert.match(html, /Free trial credit/);
  assert.match(html, /\$1\.00/);
  assert.match(html, /Other credit/);
  assert.match(html, /\$2\.00/);
  assert.match(html, /Trial credit is used first/);
  assert.doesNotMatch(render(false), /Free trial credit/);
});

test("missing deposit service and signed-out views do not offer checkout", () => {
  assert.match(render(), /Deposits are temporarily unavailable/);
  assert.doesNotMatch(render(), /checkout/);
  assert.match(render(), /Redeem a gift code/);
  assert.match(render(), /type="password"/);
  assert.doesNotMatch(render(false), /Redeem a gift code/);
  assert.match(render(false), /Sign in to view your balance/);
});
