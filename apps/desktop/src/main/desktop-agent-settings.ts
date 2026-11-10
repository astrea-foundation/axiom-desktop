import type { ConfigureDesktopAgentRequest } from "@axiom/axiom-acp-client";

export function agentRevision(value: unknown): number {
  if (!Number.isSafeInteger(value) || (value as number) < 0) throw new Error("Invalid Agent settings revision");
  return value as number;
}

export function desktopAgentRequest(value: unknown): ConfigureDesktopAgentRequest {
  if (!value || typeof value !== "object") throw new Error("Invalid Agent settings");
  const raw = value as Record<string, unknown>;
  if (typeof raw.threadId !== "string" || !raw.threadId || raw.threadId.length > 128
    || typeof raw.enabled !== "boolean"
    || !["approve_commands", "full_access"].includes(raw.permission as string)
    || (raw.workingDirectory != null && (typeof raw.workingDirectory !== "string" || !raw.workingDirectory
      || raw.workingDirectory.length > 32_768 || raw.workingDirectory.includes("\0")))) throw new Error("Invalid Agent settings");
  return { threadId: raw.threadId, enabled: raw.enabled,
    permission: raw.permission as ConfigureDesktopAgentRequest["permission"],
    expectedRevision: agentRevision(raw.expectedRevision), workingDirectory: (raw.workingDirectory as string | null) ?? null };
}
