import assert from "node:assert/strict";
import test from "node:test";
import { parseLocalMcpConfig } from "../src/renderer/src/mcpConfig";
import { desktopMcpRequest } from "../src/main/desktop-mcp-settings";

test("local MCP imports preserve argv and environment while starting disabled", () => {
  const result = parseLocalMcpConfig(JSON.stringify({ mcpServers: { "Local Files": { command: "npx", args: ["-y", "server", "/folder with spaces"], env: { API_KEY: "secret" } } } }));
  assert.deepEqual(result, [{ name: "local_files", displayName: "Local Files", command: "npx", args: ["-y", "server", "/folder with spaces"], env: { API_KEY: "secret" }, enabled: false }]);
});
test("MCP import rejects unsupported transports, ambiguous names and malformed environment", () => {
  for (const server of [{ url: "https://example.invalid/mcp", type: "sse" }, { command: "server", type: "sse" }, { command: "server", args: "--token secret" }, { command: "server", env: { TOKEN: 12 } }, { command: "server", headers: {} }]) {
    assert.throws(() => parseLocalMcpConfig(JSON.stringify({ mcpServers: { example: server } })));
  }
  assert.throws(() => parseLocalMcpConfig('{"mcpServers":{"a-b":{"command":"server"},"a_b":{"command":"server"}}}'));
});
test("MCP IPC rejects oversized data, invalid revisions and unknown actions", () => {
  for (const request of [null, [], { action: { kind: "connect_remote" } }, { action: { kind: "list" }, expectedRevision: -1 }, { action: { kind: "list" }, threadId: 2 }, { action: { kind: "import", servers: "x".repeat(1024 * 1024) } }]) assert.throws(() => desktopMcpRequest(request));
});


test("remote MCP imports isolate credentials and start disabled", () => {
  const [server] = parseLocalMcpConfig(JSON.stringify({ mcpServers: { "Project tracker": { url: "https://mcp.example.com/mcp", headers: { Authorization: "Bearer private-fixture-token" } } } }));
  assert.deepEqual(server, { name: "project_tracker", displayName: "Project tracker", command: "", args: [], enabled: false, remote: { url: "https://mcp.example.com/mcp", auth: "bearer" }, bearerToken: "private-fixture-token" });
  const [publicServer] = parseLocalMcpConfig('{"mcpServers":{"public":{"url":"https://mcp.example.com/mcp"}}}');
  assert.equal(publicServer?.remote?.auth, "none");
  for (const entry of [
    {url:"http://example.com/mcp"}, {url:"https://u:secret@example.com/mcp"}, {url:"https://example.com/mcp?token=secret"},
    {url:"https://example.com/mcp",command:"server"}, {url:"https://example.com/mcp",env:{TOKEN:"secret"}},
    {url:"https://example.com/mcp",headers:{"x-api-key":"secret"}}, {url:"https://example.com/mcp",headers:{Authorization:"Basic secret"}},
    {url:"https://example.com/mcp",auth:"none",headers:{Authorization:"Bearer secret"}},
  ]) assert.throws(() => parseLocalMcpConfig(JSON.stringify({mcpServers:{bad:entry}})));
});
