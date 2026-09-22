import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { WebConsentDialog } from "../src/renderer/src/components/WebConsentDialog";
import { rememberWebWarning, skipWebWarning } from "../src/renderer/src/webConsent";

test("warning is vendor-neutral and clearly separates web disclosure from private model traffic", () => {
  const html = renderToStaticMarkup(createElement(WebConsentDialog, { onCancel: () => {}, onConfirm: () => {} }));
  assert.match(html, /Web searches run outside Axiom’s verified private environment\./);
  assert.match(html, /Queries are sent to web search providers to retrieve results\./);
  assert.match(html, /Your model conversation remains end-to-end encrypted, but search queries may contain details from your messages\./);
  assert.match(html, /No thanks/);
  assert.match(html, />Yes, I understand<\/button>/);
  assert.match(html, /Don’t show this warning again/);
  assert.doesNotMatch(html, /Decodo|window\.confirm|checked=""/i);
});

test("warning preference is per-account, exact, and fail-closed when storage is unavailable", () => {
  const values = new Map<string, string>();
  const storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } };
  assert.equal(skipWebWarning(storage, "account-a"), false);
  rememberWebWarning(storage, "account-a");
  assert.equal(skipWebWarning(storage, "account-a"), true);
  assert.equal(skipWebWarning(storage, "account-b"), false);
  assert.deepEqual([...values.values()], ["acknowledged"]);
  assert.equal(skipWebWarning(null, "account-a"), false);
  const broken = { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } };
  assert.equal(skipWebWarning(broken, "account-a"), false);
  assert.doesNotThrow(() => rememberWebWarning(broken, "account-a"));
});
