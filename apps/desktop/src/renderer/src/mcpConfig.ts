import type { DesktopMcpServerInput, McpAuthentication } from "@axiom/axiom-acp-client";

function remoteUrl(value: unknown): string {
  if (typeof value !== "string" || value.length > 4096 || /[\p{Cc}\p{Cf}]/u.test(value)) throw new Error("Invalid remote MCP URL.");
  const url = new URL(value);
  const loopback = url.hostname === "localhost" || /^127(?:\.\d{1,3}){3}$/.test(url.hostname) || url.hostname === "[::1]";
  if (!(url.protocol === "https:" || (url.protocol === "http:" && loopback)) || url.username || url.password || url.search || url.hash) throw new Error("Use an HTTPS MCP URL without credentials, query strings or fragments. Loopback HTTP is allowed for local development.");
  return url.href;
}

/** Import reviewed stdio and Streamable HTTP connections. Secrets are write-only. */
export function parseMcpConfig(text: string): DesktopMcpServerInput[] {
  if (text.length > 1024 * 1024) throw new Error("MCP configuration is too large.");
  const config: unknown = JSON.parse(text);
  if (!config || typeof config !== "object" || Array.isArray(config)) throw new Error("Expected an mcpServers object.");
  const servers = (config as { mcpServers?: unknown }).mcpServers;
  if (!servers || typeof servers !== "object" || Array.isArray(servers)) throw new Error("Expected an mcpServers object.");
  const entries = Object.entries(servers);
  if (!entries.length || entries.length > 32) throw new Error("Import between 1 and 32 connections.");
  const names = new Set<string>();
  return entries.map(([label, entry]) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) throw new Error(`Invalid configuration for ${label}.`);
    const server = entry as Record<string, unknown>;
    const name = label.toLowerCase().replace(/[^a-z0-9_]/g, "_").replace(/^_+|_+$/g, "");
    if (!name || name.length > 48 || names.has(name)) throw new Error("Server names must be unique after converting to lowercase letters, numbers and underscores.");
    names.add(name);
    if (server.url !== undefined) {
      if (Object.keys(server).some(key => !["url", "type", "transport", "headers", "auth", "clientId", "clientMetadataUrl", "disabled"].includes(key))) throw new Error(`Unsupported remote fields in ${label}. Local commands and environment variables cannot be mixed with a remote URL.`);
      for (const transport of [server.type, server.transport]) if (transport !== undefined && !["http", "streamable-http", "streamable_http"].includes(transport as string)) throw new Error("Remote connections use Streamable HTTP. Legacy SSE is not supported.");
      let auth: McpAuthentication = "none";
      let bearerToken: string | undefined;
      if (server.headers !== undefined) {
        if (!server.headers || typeof server.headers !== "object" || Array.isArray(server.headers)) throw new Error("Remote headers must be an object.");
        const headers = Object.entries(server.headers);
        if (headers.length !== 1 || headers[0]![0].toLowerCase() !== "authorization" || typeof headers[0]![1] !== "string") throw new Error("Only a bearer Authorization header can be imported. Other custom headers are not supported.");
        const token = /^Bearer ([^\r\n]+)$/i.exec(headers[0]![1]);
        if (!token || !token[1] || token[1].length > 16_384) throw new Error("Authorization must contain a bearer token.");
        auth = "bearer"; bearerToken = token[1];
      }
      if (server.auth !== undefined) {
        if (!["none", "bearer", "oauth"].includes(server.auth as string) || (bearerToken && server.auth !== "bearer")) throw new Error("Invalid remote authentication settings.");
        auth = server.auth as McpAuthentication;
      }
      if ((server.clientId !== undefined && typeof server.clientId !== "string") || (server.clientMetadataUrl !== undefined && typeof server.clientMetadataUrl !== "string")) throw new Error("OAuth client settings must be strings.");
      if (auth !== "oauth" && (server.clientId !== undefined || server.clientMetadataUrl !== undefined)) throw new Error("Client settings require OAuth authentication.");
      return { name, displayName: label, command: "", args: [], enabled: false, remote: { url: remoteUrl(server.url), auth,
        ...(server.clientId ? { clientId: server.clientId as string } : {}), ...(server.clientMetadataUrl ? { clientMetadataUrl: remoteUrl(server.clientMetadataUrl) } : {}) }, ...(bearerToken ? { bearerToken } : {}) };
    }
    if ((server.type !== undefined && server.type !== "stdio") || (server.transport !== undefined && server.transport !== "stdio")) throw new Error("A remote connection needs a URL; local connections use stdio.");
    if (Object.keys(server).some(key => !["command", "args", "env", "type", "transport", "disabled"].includes(key))) throw new Error(`Unsupported fields in ${label}. Use command, args and env for a local server.`);
    if (typeof server.command !== "string" || !server.command.trim()) throw new Error(`Missing command for ${label}.`);
    if (server.args !== undefined && (!Array.isArray(server.args) || server.args.some(arg => typeof arg !== "string"))) throw new Error(`Arguments for ${label} must be a string array.`);
    if (server.env !== undefined && (!server.env || typeof server.env !== "object" || Array.isArray(server.env) || Object.values(server.env).some(value => typeof value !== "string"))) throw new Error(`Environment variables for ${label} must be strings.`);
    return { name, displayName: label, command: server.command, args: server.args as string[] ?? [], env: server.env as Record<string, string> | undefined, enabled: false };
  });
}

/** Compatibility for hosts that still use the original import function name. */
export const parseLocalMcpConfig = parseMcpConfig;
