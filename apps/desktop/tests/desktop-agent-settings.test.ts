import assert from "node:assert/strict";
import test from "node:test";
import { agentRevision, desktopAgentRequest } from "../src/main/desktop-agent-settings";

test("Agent IPC validates explicit permissions and safe revision counters", () => {
  const request = { threadId: "thread", expectedRevision: 0, enabled: false, permission: "approve_commands", workingDirectory: null };
  assert.deepEqual(desktopAgentRequest(request), request);
  for (const invalid of [null, {}, { ...request, enabled: "true" }, { ...request, permission: "confirm" },
    { ...request, workingDirectory: "x\0y" }, { ...request, expectedRevision: -1 }, { ...request, expectedRevision: 0.5 }]) {
    assert.throws(() => desktopAgentRequest(invalid), /Invalid Agent/);
  }
  for (const invalid of [undefined, null, "0", Infinity, Number.MAX_SAFE_INTEGER + 1]) assert.throws(() => agentRevision(invalid));
});

test("Agent IPC preserves platform-native Unicode folder paths without rewriting them", () => {
  for (const workingDirectory of ["C:\\Users\\Tester\\My Project 日本語", "\\\\server\\share\\Project", "/Users/test/My Project", "/home/test/My Project"]) {
    const result = desktopAgentRequest({ threadId: "thread", expectedRevision: 3, enabled: true, permission: "full_access", workingDirectory });
    assert.equal(result.workingDirectory, workingDirectory);
  }
});
