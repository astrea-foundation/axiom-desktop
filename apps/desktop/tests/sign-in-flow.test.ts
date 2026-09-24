import assert from "node:assert/strict";
import test from "node:test";
import { accountPresentation, desktopErrorMessage, signInErrorMessage } from "../src/renderer/src/signInFlow.js";

test("account presentation distinguishes startup, sign-in, revocation, and outages", () => {
  assert.equal(accountPresentation(false, false, null).kind, "starting");
  assert.equal(accountPresentation(true, true, null).kind, "signed-out");
  assert.deepEqual(
    accountPresentation(true, true, {
      revision: 3,
      state: "expired",
      detail: "This session was revoked.",
    }),
    {
      kind: "expired",
      title: "Reconnect your Axiom account",
      detail: "This session was revoked.",
    },
  );
  assert.equal(accountPresentation(true, true, {
    revision: 4,
    state: "unavailable",
    detail: "Service is unreachable.",
  }).kind, "unavailable");
  assert.equal(accountPresentation(true, true, {
      revision: 5,
      state: "valid",
      account: {
        id: "account-id",
        displayName: "Ada",
        verifiedEmail: "ada@example.test",
        linkedMethods: ["passkey", "google"],
      },
      session: { expiresAt: "2030-01-01T00:00:00Z", credentialStore: "system keyring" },
    }).kind, "valid");
});

test("sign-in errors remove IPC wrapping while preserving useful detail", () => {
  assert.equal(
    signInErrorMessage(new Error(
      "Error invoking remote method 'agent:native-login-complete': AcpError: authorization expired",
    )),
    "authorization expired",
  );
  assert.match(signInErrorMessage(null), /could not be completed/i);
  assert.equal(
    desktopErrorMessage(
      new Error("Error invoking remote method 'agent:models-list': Error: catalog mismatch"),
      "fallback",
    ),
    "catalog mismatch",
  );
});
