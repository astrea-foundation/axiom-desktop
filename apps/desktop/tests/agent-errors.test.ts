import assert from "node:assert/strict";
import test from "node:test";
import { AcpError } from "@axiom/axiom-acp-client";
import { actionableAcpError } from "../src/main/agent-errors";

test("native delete errors retain their actionable reason across Electron IPC", () => {
  const error = new AcpError(-32603, "Internal error", "sessions with active work must be cancelled before deletion");
  assert.equal(actionableAcpError(error).message, "sessions with active work must be cancelled before deletion");
});

test("structured or empty error data is not reflected as an error message", () => {
  assert.equal(actionableAcpError(new AcpError(-32603, "Internal error", { secret: "not displayable" })).message, "Internal error");
  assert.equal(actionableAcpError(new AcpError(-32603, "Internal error", "  ")).message, "Internal error");
  assert.equal(actionableAcpError(new Error("Disconnected")).message, "Disconnected");
});
