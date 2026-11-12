import assert from "node:assert/strict";
import test from "node:test";
import { AttachmentDraftWrites } from "../src/renderer/src/attachmentDraftWrites";
import type { PayloadStore, PromptPayload } from "@axiom/chat-core/localPayloads";
const files = (name: string) => [{ kind: "file" as const, name, file: { name, mimeType: "text/plain", data: "eA==" } }];

test("dirty attachment writes retry when the owning account returns and preserve the latest array", async () => {
  let owner = "alice", fail = true;
  const saved = new Map<string, PromptPayload>();
  const store: PayloadStore = {
    get: async () => undefined, remove: async () => {},
    put: async (account, id, value) => { if (fail || account !== owner) throw Error("not saved"); saved.set(id, value); },
  };
  const writes = new AttachmentDraftWrites(store);
  await assert.rejects(writes.write("alice", "draft", files("first.txt")));
  owner = "bob"; fail = false;
  await assert.rejects(writes.retry("alice", "draft"));
  await assert.rejects(writes.write("alice", "draft", files("latest.txt")));
  owner = "alice";
  await writes.retry("alice", "draft");
  assert.equal(saved.get("draft")?.attachments[0]?.name, "latest.txt");
});

test("an old attachment acknowledgement cannot clear a newer failed edit", async () => {
  let complete!: () => void;
  let first = true, retry = false;
  const saved: PromptPayload[] = [];
  const writes = new AttachmentDraftWrites({ get: async () => undefined, remove: async () => {},
    put: async (_, __, value) => {
      if (first) { first = false; await new Promise<void>(resolve => { complete = resolve; }); }
      else if (!retry) throw Error("latest edit failed");
      saved.push(value);
    },
  });
  const old = writes.write("alice", "draft", files("old.txt"));
  await assert.rejects(writes.write("alice", "draft", files("new.txt")));
  complete(); await old;
  retry = true; await writes.retry("alice", "draft");
  assert.equal(saved.at(-1)?.attachments[0]?.name, "new.txt");
});

test("account transition flushes only its current owner and retains older dirty attachments for that owner's return", async () => {
  let owner = "bob", enabled = false;
  const saved: string[] = [];
  const writes = new AttachmentDraftWrites({ get: async () => undefined, remove: async () => {},
    put: async (account) => { if (!enabled || account !== owner) throw Error("owner unavailable"); saved.push(account); },
  });
  await assert.rejects(writes.write("alice", "draft", files("alice.txt")));
  await assert.rejects(writes.write("bob", "draft", files("bob.txt")));
  enabled = true;
  await writes.flush("bob");
  assert.deepEqual(saved, ["bob"]);
  await assert.rejects(writes.flush(), /owner unavailable/);
  owner = "alice"; await writes.retry("alice", "draft");
  assert.deepEqual(saved, ["bob", "alice"]);
});
