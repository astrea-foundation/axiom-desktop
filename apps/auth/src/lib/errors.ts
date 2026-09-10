import { ApiError } from "../api/client";

export function readableError(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof DOMException && error.name === "NotAllowedError") {
    return "The request was cancelled or timed out. Nothing was changed.";
  }
  if (error instanceof Error && SAFE_MESSAGES.has(error.message)) return error.message;
  return "The action could not be completed. Please try again.";
}

const SAFE_MESSAGES = new Set([
  "That code is incorrect, expired, or no longer available. Try the latest email or start sign-in again.",
  "Passkeys are not available in this browser.",
  "No compatible Ethereum wallet was found in this browser.",
  "The wallet did not return an account.",
  "The wallet returned an invalid chain identifier.",
  "The wallet returned an invalid address.",
  "The wallet returned an invalid signature.",
  "The sign-in message is invalid.",
  "Unsupported Ethereum chain.",
]);
