import assert from "node:assert/strict";
import test from "node:test";
import { forgetThreadWebAccess, readThreadWebAccess, saveThreadWebAccess } from "../src/renderer/src/webAccessStorage";
import { rememberWebWarning, skipWebWarning } from "../src/renderer/src/webConsent";

function fixture() {
  const values = new Map<string, string>();
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
    removeItem: (key: string) => { values.delete(key); },
  };
}

test("Web choices persist independently per account and thread, including explicit off", () => {
  const storage = fixture();
  assert.equal(readThreadWebAccess(storage, "a", "chat"), false);
  assert.equal(saveThreadWebAccess(storage, "a", "chat", true), true);
  assert.equal(readThreadWebAccess(storage, "a", "chat"), true);
  assert.equal(readThreadWebAccess(storage, "a", "other"), false);
  assert.equal(readThreadWebAccess(storage, "b", "chat"), false);
  assert.equal(skipWebWarning(storage, "a"), false);
  rememberWebWarning(storage, "b");
  assert.equal(readThreadWebAccess(storage, "b", "chat"), false);
  assert.equal(saveThreadWebAccess(storage, "a", "chat", false), true);
  assert.equal(readThreadWebAccess(storage, "a", "chat"), false);
  assert.equal(skipWebWarning(storage, "b"), true);
});

test("drafts and invalid scopes cannot become saved Web defaults; composite keys cannot collide", () => {
  const storage = fixture();
  for (const [account, thread] of [["a", "new"], ["", "chat"], ["a", ""]]) {
    assert.equal(saveThreadWebAccess(storage, account!, thread!, true), false);
    assert.equal(readThreadWebAccess(storage, account!, thread!), false);
  }
  assert.equal(storage.values.size, 0);
  saveThreadWebAccess(storage, "a:b", "c", true);
  assert.equal(readThreadWebAccess(storage, "a", "b:c"), false);
});

test("only exact saved opt-in enables Web; unavailable storage fails closed", () => {
  for (const value of [null, "", "true", "1", "acknowledged", "disabled", "Enabled", '{"enabled":true}']) {
    assert.equal(readThreadWebAccess({ getItem: () => value, setItem: () => {} }, "a", "chat"), false);
  }
  const broken = { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("full"); } };
  assert.equal(readThreadWebAccess(broken, "a", "chat"), false);
  assert.equal(readThreadWebAccess(null, "a", "chat"), false);
  assert.equal(saveThreadWebAccess(broken, "a", "chat", true), false);
  assert.equal(saveThreadWebAccess(null, "a", "chat", false), false);
  assert.equal(saveThreadWebAccess(fixture(), "a", "chat", "true" as unknown as boolean), false);
});

test("deleting a thread removes only its account-scoped Web preference", () => {
  const storage = fixture();
  saveThreadWebAccess(storage, "a", "chat", true);
  saveThreadWebAccess(storage, "b", "chat", true);
  rememberWebWarning(storage, "a");
  forgetThreadWebAccess(storage, "a", "chat");
  assert.equal(readThreadWebAccess(storage, "a", "chat"), false);
  assert.equal(readThreadWebAccess(storage, "b", "chat"), true);
  assert.equal(skipWebWarning(storage, "a"), true);
});
