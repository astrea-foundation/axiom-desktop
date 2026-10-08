import type { DesktopMcpServerInput } from "@axiom/axiom-acp-client";

/** Import the common local mcpServers format. Remote URLs are explicitly rejected. */
export function parseLocalMcpConfig(text: string): DesktopMcpServerInput[] {
  if (text.length > 1024 * 1024) throw new Error("MCP configuration is too large.");
  const config: unknown = JSON.parse(text);
  if (!config || typeof config !== "object" || Array.isArray(config)) throw new Error("Expected an mcpServers object.");
  const servers = (config as { mcpServers?: unknown }).mcpServers;
  if (!servers || typeof servers !== "object" || Array.isArray(servers)) throw new Error("Expected an mcpServers object.");
  const entries = Object.entries(servers);
  if (!entries.length || entries.length > 32) throw new Error("Import between 1 and 32 local servers.");
  const names = new Set<string>();
  return entries.map(([label, entry]) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) throw new Error(`Invalid configuration for ${label}.`);
    const server = entry as Record<string, unknown>;
    if (server.url !== undefined || (server.type !== undefined && server.type !== "stdio") || server.transport !== undefined && server.transport !== "stdio") throw new Error("Only local stdio MCP servers are supported.");
    if (Object.keys(server).some((key) => !["command", "args", "env", "type", "transport", "disabled"].includes(key))) throw new Error(`Unsupported fields in ${label}. Use command, args and env for a local server.`);
    const name = label.toLowerCase().replace(/[^a-z0-9_]/g, "_").replace(/^_+|_+$/g, "");
    if (!name || name.length > 48 || names.has(name)) throw new Error("Server names must be unique after converting to lowercase letters, numbers and underscores.");
    names.add(name);
    if (typeof server.command !== "string" || !server.command.trim()) throw new Error(`Missing command for ${label}.`);
    if (server.args !== undefined && (!Array.isArray(server.args) || server.args.some((arg) => typeof arg !== "string"))) throw new Error(`Arguments for ${label} must be a string array.`);
    if (server.env !== undefined && (!server.env || typeof server.env !== "object" || Array.isArray(server.env) || Object.values(server.env).some((value) => typeof value !== "string"))) throw new Error(`Environment variables for ${label} must be strings.`);
    return { name, command: server.command, args: server.args as string[] ?? [], env: server.env as Record<string, string> | undefined, enabled: false };
  });
}
