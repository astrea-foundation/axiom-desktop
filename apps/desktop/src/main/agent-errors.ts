import { AcpError } from "@axiom/axiom-acp-client";

/** Electron IPC serializes Error.message, not ACP's separate error data. */
export function actionableAcpError(error: unknown): Error {
  if (error instanceof AcpError && typeof error.data === "string" && error.data.trim()) {
    return new Error(error.data.trim());
  }
  return error instanceof Error ? error : new Error(String(error));
}
