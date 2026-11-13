/** Presentation only: these errors still fail the reply's completion checks. */
export function replyErrorPresentation(detail: string): { message: string; interrupted: boolean } | null {
  switch (detail.trim()) {
    case "Tinfoil stream ended without authenticated completion":
    case "relay stream ended without terminal completion":
    case "provider stream ended before [DONE]":
      return { message: "The reply ended early. Try again.", interrupted: true };
    case "Tinfoil response stalled":
    case "Tinfoil response timed out":
      return { message: "The reply timed out. Try again.", interrupted: true };
    case "Tinfoil request timed out":
      return { message: "The request timed out. Try again.", interrupted: true };
    case "Tinfoil reported an inference error":
      return { message: "The model couldn't finish this reply. Try again.", interrupted: true };
    // The native error intentionally combines transport and authentication
    // failures. Do not guess that these are ordinary network interruptions.
    case "Tinfoil response authentication or transport failed":
    case "Tinfoil encrypted exchange failed":
      return { message: "Couldn't complete this reply securely. Try again.", interrupted: false };
    default:
      return null;
  }
}
