import assert from "node:assert/strict";
import test from "node:test";
import { accountPresentation, desktopErrorMessage, signInErrorMessage, signInErrorPresentation } from "../src/renderer/src/signInFlow.js";

test("account presentation distinguishes startup, sign-in, revocation, and outages", () => {
  assert.equal(accountPresentation(false, false, null).kind, "starting");
  assert.equal(accountPresentation(true, true, null).kind, "starting");
  assert.equal(accountPresentation(true, true, { revision: 1, state: "signed_out" }).kind, "signed-out");
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

test("native preparation shows the actual operation without an outage or sign-in prompt", () => {
  const checking = accountPresentation(true, true, { revision: 1, state: "starting", preparationPhase: "checking_sign_in" });
  assert.equal(checking.kind, "starting");
  assert.equal(checking.title, "Checking your sign-in…");
  const opening = accountPresentation(true, true, { revision: 2, state: "starting", preparationPhase: "opening_chats" });
  assert.equal(opening.kind, "starting");
  assert.equal(opening.title, "Unlocking your chats");
  assert.match(opening.detail, /key for your encrypted chats/);
  assert.doesNotMatch(opening.detail, /decrypting|unavailable/i);
  const decrypting = accountPresentation(true, true, { revision: 3, state: "starting", preparationPhase: "decrypting_chats" });
  assert.equal(decrypting.title, "Decrypting your chats");
  assert.match(decrypting.detail, /on this device/);
  const failed = accountPresentation(true, true, { revision: 3, state: "unavailable", preparationPhase: "opening_chats", detail: "Synthetic key access was denied" });
  assert.equal(failed.kind, "unavailable");
  assert.equal(failed.title, "Couldn’t open your chats");
  assert.equal(failed.kind === "unavailable" && failed.technicalDetails, "Synthetic key access was denied");
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

test("invalid TLS data while starting sign-in explains the stage and preserves diagnostics", () => {
  const nativeError = "provider error: could not start login: error sending request: client error (Connect): received corrupt message of type InvalidContentType";
  const message = signInErrorMessage(new Error(
    `Error invoking remote method 'agent:native-login-start': AcpError: ${nativeError}`,
  ));
  const error = signInErrorPresentation(message);
  assert.equal(error.title, "Couldn’t connect securely");
  assert.match(error.message, /invalid response.*secure connection/);
  assert.match(error.message, /browser sign-in could not start/);
  assert.match(error.message, /VPN, proxy, or network filter/);
  assert.match(error.message, /another network.*phone hotspot/);
  assert.doesNotMatch(error.message, /InvalidContentType|provider error/);
  assert.equal(error.technicalDetails, nativeError);
});

test("other sign-in failures retain their reason without assuming invalid TLS or a failed start", () => {
  for (const message of [
    "authorization expired",
    "could not start login: operation timed out",
    "could not start login: invalid peer certificate: UnknownIssuer",
    "Axiom rejected the login request with HTTP 503 Service Unavailable",
    "could not complete login: received corrupt message of type InvalidContentType",
    "could not start login: invalid HTTP Content-Type",
  ]) {
    assert.deepEqual(signInErrorPresentation(message), {
      title: "Sign-in didn’t finish",
      message,
    });
  }
});
