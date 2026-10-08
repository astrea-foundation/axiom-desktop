import type { DesktopMcpRequest } from "@axiom/axiom-acp-client";

/** Native ACP performs semantic validation; IPC bounds write-only secret payloads. */
export function desktopMcpRequest(value: unknown): DesktopMcpRequest {
  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Invalid MCP request");
  const raw = value as DesktopMcpRequest;
  if (JSON.stringify(value).length > 1024 * 1024 || !raw.action || typeof raw.action !== "object"
    || !["list", "save", "delete", "test", "select", "import"].includes(raw.action.kind)) throw new Error("Invalid MCP action");
  if (raw.threadId != null && (typeof raw.threadId !== "string" || raw.threadId.length > 128)) throw new Error("Invalid MCP thread");
  if (raw.expectedRevision != null && (!Number.isSafeInteger(raw.expectedRevision) || raw.expectedRevision < 0)) throw new Error("Invalid MCP revision");
  return raw;
}
