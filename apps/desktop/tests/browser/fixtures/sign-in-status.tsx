import { useState } from "react";
import { createRoot } from "react-dom/client";
import { SignInStatus } from "../../../src/renderer/src/components/SignInStatus";
import { signInErrorMessage, type SignInState } from "../../../src/renderer/src/signInFlow";
import "../../../src/renderer/src/index.css";

const nativeError = "provider error: could not start login: error sending request: client error (Connect): received corrupt message of type InvalidContentType";

function SignInHarness() {
  const expired = new URLSearchParams(window.location.search).get("scenario") === "expired";
  const [state, setState] = useState<SignInState>({
    kind: "error",
    message: signInErrorMessage(new Error(
      `Error invoking remote method 'agent:native-login-start': AcpError: ${expired ? "authorization expired" : nativeError}`,
    )),
  });
  return <div className="bg-[var(--color-bg-base)] p-5" style={{ minHeight: "100vh" }}>
    <main className="rounded-[22px] bg-[var(--surface-stage)] p-5 shadow-glass-card" style={{ minHeight: 280 }}>
      <header className="mb-5 text-[13px] font-medium text-[var(--color-text-primary)]">New thread</header>
      {state.kind === "closed" ? <p>Sign-in status dismissed</p> : (
        <SignInStatus state={state} onRetry={() => setState({ kind: "starting" })}
          onCancel={() => setState({ kind: "closed" })} />
      )}
    </main>
  </div>;
}

createRoot(document.getElementById("root")!).render(<SignInHarness />);
