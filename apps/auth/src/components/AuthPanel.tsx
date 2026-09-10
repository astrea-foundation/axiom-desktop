import { useEffect, useState, type FormEvent } from "react";

import { ApiError, authApi } from "../api/client";
import type { EmailLoginChallenge } from "../api/types";
import { connectEthereumWallet, signSiweMessage } from "../lib/ethereum";
import { readableError } from "../lib/errors";
import { authenticateWithPasskey, registerPasskey } from "../lib/webauthn";
import { Fingerprint, Wallet, ArrowRight } from "lucide-react";
import { GoogleIcon } from "./Icons";
import { Notice, Spinner } from "./Layout";

type AuthPanelMode = "signin" | "signup" | "link-password";

interface AuthPanelProps {
  mode: AuthPanelMode;
  returnTo: string;
  onAuthenticated(): Promise<void>;
}

export function AuthPanel({ mode, returnTo, onAuthenticated }: AuthPanelProps) {
  const [emailMode, setEmailMode] = useState(true);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [challenge, setChallenge] = useState<EmailLoginChallenge | null>(null);
  const [code, setCode] = useState("");
  const [resendAt, setResendAt] = useState(0);
  const [now, setNow] = useState(Date.now());
  const [displayName, setDisplayName] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<{ kind: "success" | "error"; text: string } | null>(null);

  const signup = mode !== "signin";
  const linkPassword = mode === "link-password";
  useEffect(() => {
    if (!challenge) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [challenge]);

  function acceptChallenge(value: EmailLoginChallenge) {
    setChallenge(value);
    setCode("");
    setNow(Date.now());
    setResendAt(Date.now() + value.resend_after_seconds * 1000);
  }
  async function perform(name: string, action: () => Promise<void>) {
    if (busy) return;
    setBusy(name);
    setNotice(null);
    try {
      await action();
    } catch (error) {
      setNotice({ kind: "error", text: readableError(error) });
    } finally {
      setBusy(null);
    }
  }

  async function submitPassword(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await perform("email", async () => {
      if (signup) {
        await authApi.passwordSignup({
          email,
          display_name: displayName.trim() || undefined,
          intent: linkPassword ? "link" : "signup",
        });
        setNotice({
          kind: "success",
          text: linkPassword
            ? "Check your email, then choose a password to finish linking this sign-in method."
            : "Check your email, then choose a password to finish creating your account. The link expires soon.",
        });
      } else {
        acceptChallenge(await authApi.passwordLogin(email, password));
        setPassword("");
      }
    });
  }

  async function submitCode(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!challenge) return;
    await perform("code", async () => {
      try {
        await authApi.verifyLoginCode(challenge.challenge_id, code);
      } catch (error) {
        setCode("");
        if (error instanceof ApiError && error.status === 401) {
          throw new Error("That code is incorrect, expired, or no longer available. Try the latest email or start sign-in again.");
        }
        throw error;
      }
      setCode("");
      await onAuthenticated();
    });
  }

  if (challenge) {
    const resendWait = Math.max(0, Math.ceil((resendAt - now) / 1000));
    const expired = Date.parse(challenge.expires_at) <= now;
    return (
      <div className="auth-panel">
        <p className="lead-small">Enter the six-digit sign-in code sent to <strong>{email}</strong>.</p>
        <form className="password-form" onSubmit={(event) => void submitCode(event)}>
          <label className="field"><span>Email sign-in code</span>
            <input type="text" inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}"
              maxLength={6} minLength={6} required autoFocus value={code}
              onChange={(event) => setCode(event.target.value.replace(/[^0-9]/g, ""))} />
          </label>
          <p className="field-help">{expired ? "This code has expired. Start sign-in again to get a new one." : "Your code expires in 10 minutes. Use it in this browser and never share it."}</p>
          <button className="action-button" type="submit" disabled={Boolean(busy) || expired}>
            {busy === "code" && <Spinner />} Verify and sign in
          </button>
        </form>
        <button className="text-button full-width" type="button" disabled={Boolean(busy) || resendWait > 0 || expired}
          onClick={() => void perform("resend", async () => {
            acceptChallenge(await authApi.resendLoginCode(challenge.challenge_id));
            setNotice({ kind: "success", text: "A new code is on its way. Use the code from the latest email." });
          })}>
          {busy === "resend" ? "Sending…" : resendWait > 0 ? `Resend code in ${resendWait}s` : "Resend code"}
        </button>
        <button className="text-button full-width" type="button" disabled={Boolean(busy)} onClick={() => {
          setChallenge(null); setCode(""); setPassword(""); setNotice(null);
        }}>Start sign-in again</button>
        {notice && <Notice kind={notice.kind}>{notice.text}</Notice>}
      </div>
    );
  }

  async function usePasskey() {
    await perform("passkey", async () => {
      if (mode === "signin") {
        const options = await authApi.passkeyAuthenticationOptions();
        const credential = await authenticateWithPasskey(options);
        await authApi.verifyPasskeyAuthentication(options.challenge_id, credential);
      } else {
        const intent = linkPassword ? "link" : "signup";
        const options = await authApi.passkeyRegistrationOptions(intent, displayName.trim() || undefined);
        const credential = await registerPasskey(options);
        await authApi.verifyPasskeyRegistration(options.challenge_id, credential, intent);
      }
      await onAuthenticated();
    });
  }

  async function useWallet() {
    await perform("wallet", async () => {
      const connection = await connectEthereumWallet();
      const intent = "signin" as const;
      const challenge = await authApi.siweChallenge(connection.address, connection.chainId, intent);
      const signature = await signSiweMessage(connection, challenge.message);
      await authApi.verifySiwe(challenge.challenge_id, challenge.message, signature, intent);
      await onAuthenticated();
    });
  }

  function useGoogle() {
    if (busy) return;
    setBusy("google");
    window.location.assign(authApi.googleStartUrl(returnTo));
  }

  if (linkPassword) {
    return (
      <div className="auth-panel">
        <PasswordForm
          signup
          link
          email={email}
          password={password}
          busy={busy === "email"}
          onEmail={setEmail}
          onPassword={setPassword}
          onSubmit={submitPassword}
        />
        {notice && <Notice kind={notice.kind}>{notice.text}</Notice>}
      </div>
    );
  }

  return (
    <div className="auth-panel">
      {mode === "signup" && (
        <label className="field">
          <span>Display name <small>optional</small></span>
          <input
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
            autoComplete="name"
            maxLength={80}
            placeholder="How should we address you?"
          />
        </label>
      )}

      <div className="provider-stack">
        <button className="provider-button" type="button" onClick={useGoogle} disabled={Boolean(busy)}>
          {busy === "google" ? <Spinner /> : <GoogleIcon />}<span>Google</span>
        </button>
        <button className="provider-button" type="button" onClick={() => void usePasskey()} disabled={Boolean(busy)}>
          {busy === "passkey" ? <Spinner /> : <Fingerprint />}<span>Passkey</span>
        </button>
        <button className="provider-button ethereum-button" type="button" onClick={() => void useWallet()} disabled={Boolean(busy)}>
          {busy === "wallet" ? <Spinner /> : <Wallet />}<span>Continue with Ethereum</span>
        </button>
      </div>
      <p className="fine-print">Your wallet is used only to sign in. No payment is requested.</p>

      <div className="divider"><span>or continue with email</span></div>

      {!emailMode ? (
        <button className="text-button full-width" type="button" onClick={() => setEmailMode(true)}>
          Continue with email and password
        </button>
      ) : (
        <PasswordForm
          signup={signup}
          email={email}
          password={password}
          busy={busy === "email"}
          onEmail={setEmail}
          onPassword={setPassword}
          onSubmit={submitPassword}
        />
      )}

      {notice && <Notice kind={notice.kind}>{notice.text}</Notice>}

      <p className="switch-auth">
        {mode === "signin" ? (
          <>New to Axiom? <a href={`/signup${returnQuery(returnTo)}`}>Create an account</a></>
        ) : (
          <>Already have an account? <a href={`/signin${returnQuery(returnTo)}`}>Sign in</a></>
        )}
      </p>
    </div>
  );
}

interface PasswordFormProps {
  signup: boolean;
  link?: boolean;
  email: string;
  password: string;
  busy: boolean;
  onEmail(value: string): void;
  onPassword(value: string): void;
  onSubmit(event: FormEvent<HTMLFormElement>): void;
}

function PasswordForm(props: PasswordFormProps) {
  return (
    <form className="password-form" onSubmit={props.onSubmit}>
      <label className="field">
        <span>Email address</span>
        <input
          type="email"
          inputMode="email"
          value={props.email}
          onChange={(event) => props.onEmail(event.target.value)}
          autoComplete={props.signup ? "email" : "username"}
          maxLength={320}
          placeholder="you@example.com"
          required
        />
      </label>
      {!props.signup && (
        <label className="field">
          <span>Password</span>
          <input
            type="password"
            value={props.password}
            onChange={(event) => props.onPassword(event.target.value)}
            autoComplete="current-password"
            minLength={12}
            maxLength={1024}
            required
          />
        </label>
      )}
      {!props.signup && <a className="forgot-link" href="/recover">Forgot password?</a>}
      <button className="action-button" type="submit" disabled={props.busy}>
        {props.busy && <Spinner />}
        {props.link ? "Send linking email" : props.signup ? "Create account" : "Sign in"}<ArrowRight size={15} />
      </button>
    </form>
  );
}

function returnQuery(returnTo: string): string {
  return returnTo === "/account" || returnTo === "/" ? "" : `?return_to=${encodeURIComponent(returnTo)}`;
}
