import { useEffect, useState, type FormEvent } from "react";

import { authApi } from "../api/client";
import { readableError } from "../lib/errors";
import { ArrowLeft, ArrowRight, Mail, KeyRound, Check } from "lucide-react";
import { Card, Notice, Spinner } from "./Layout";

export function RecoverPage() {
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [complete, setComplete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      await authApi.requestPasswordReset(email);
      setComplete(true);
    } catch (reason) {
      setError(readableError(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <div className={`status-symbol ${complete ? "success" : ""}`}>{complete ? <Check size={23} /> : <Mail size={23} />}</div>
      <p className="eyebrow">ACCOUNT RECOVERY</p>
      <h1>{complete ? "Check your inbox." : "A fresh start."}</h1>
      <p className="lead-small">Enter your email and we’ll send a link to reset your password, if there’s an account to recover.</p>
      {complete ? (
        <Notice kind="success">If this email is linked to an account, a recovery link is on its way. Check your spam folder, too.</Notice>
      ) : (
        <form className="password-form" onSubmit={(event) => void submit(event)}>
          <label className="field"><span>Email address</span><input type="email" placeholder="you@example.com" inputMode="email" autoComplete="email" required maxLength={320} value={email} onChange={(event) => setEmail(event.target.value)} /></label>
          <button className="action-button" type="submit" disabled={busy}>{busy && <Spinner />} Send recovery link <ArrowRight size={15} /></button>
        </form>
      )}
      {error && <Notice kind="error">{error}</Notice>}
      <p className="switch-auth"><a href="/signin"><ArrowLeft size={13} /> Back to sign in</a></p>
    </Card>
  );
}

export function ResetPasswordPage({ token }: { token: string | null }) {
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState(false);
  const [complete, setComplete] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(clearFragment, []);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!token || busy) return;
    if (password !== confirmation) {
      setError("Passwords do not match.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await authApi.resetPassword(token, password);
      setPassword("");
      setConfirmation("");
      setComplete(true);
    } catch (reason) {
      setError(readableError(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <div className={`status-symbol ${complete ? "success" : ""}`}>{complete ? <Check size={23} /> : <KeyRound size={23} />}</div>
      <p className="eyebrow">ACCOUNT RECOVERY</p>
      <h1>{complete ? "A fresh start." : "Make it yours."}</h1>
      <p className="lead-small">{complete ? "Your new password is ready to use." : "Choose a long, unique password for your account."}</p>
      {!token && <Notice kind="error">This password-reset link is incomplete or invalid.</Notice>}
      {complete ? (
        <><Notice kind="success">Your password was changed and previous sessions were revoked. We’ll email you a confirmation. Sign in with your new password and an email code.</Notice><a className="button-link full-width" href="/signin">Continue to sign in</a></>
      ) : token ? (
        <form className="password-form" onSubmit={(event) => void submit(event)}>
          <label className="field"><span>New password</span><input type="password" autoComplete="new-password" minLength={12} maxLength={1024} required value={password} onChange={(event) => setPassword(event.target.value)} /></label>
          <label className="field"><span>Confirm new password</span><input type="password" autoComplete="new-password" minLength={12} maxLength={1024} required value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
          <p className="field-help">Use a long, unique password. Password-manager paste is supported.</p>
          <button className="action-button" type="submit" disabled={busy}>{busy && <Spinner />} Change password</button>
        </form>
      ) : null}
      {error && <Notice kind="error">{error}</Notice>}
      {!complete && <p className="switch-auth"><a href="/recover">Request a new link</a></p>}
    </Card>
  );
}

export function VerifyEmailPage({ token }: { token: string | null }) {
  const [password, setPassword] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [busy, setBusy] = useState(false);
  const [complete, setComplete] = useState(false);
  const [linked, setLinked] = useState(false);
  const [error, setError] = useState(token ? null : "This verification link is incomplete or invalid.");

  useEffect(clearFragment, []);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!token || busy) return;
    if (password !== confirmation) {
      setError("Passwords do not match.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      // Load an optional browser session so a link enrollment carries its in-memory CSRF proof.
      const session = await authApi.session();
      await authApi.verifyEmail(token, password);
      setPassword("");
      setConfirmation("");
      setLinked(session.authenticated);
      setComplete(true);
    } catch (reason) {
      setError(readableError(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Card>
      <div className={`status-symbol ${complete ? "success" : ""}`}>{complete ? <Check size={23} /> : <Mail size={23} />}</div>
      <p className="eyebrow">EMAIL VERIFICATION</p>
      <h1>{complete ? "You’re ready." : "The finishing touch."}</h1>
      <p className="lead-small">{complete ? "Your email sign-in is ready. Welcome to Axiom." : "Choose a password to finish verifying your email."}</p>
      {complete ? (
        <>
          <Notice kind="success">Your email sign-in is ready to use.</Notice>
          <a className="button-link full-width" href={linked ? "/account" : "/signin"}>
            {linked ? "Continue to your account" : "Continue to sign in"}
          </a>
        </>
      ) : token ? (
        <form className="password-form" onSubmit={(event) => void submit(event)}>
          <label className="field"><span>New password</span><input type="password" autoComplete="new-password" minLength={12} maxLength={1024} required value={password} onChange={(event) => setPassword(event.target.value)} /></label>
          <label className="field"><span>Confirm password</span><input type="password" autoComplete="new-password" minLength={12} maxLength={1024} required value={confirmation} onChange={(event) => setConfirmation(event.target.value)} /></label>
          <p className="field-help">Use a long, unique password. It is stored only after this email proof succeeds.</p>
          <button className="action-button" type="submit" disabled={busy}>{busy && <Spinner />} Finish verification</button>
        </form>
      ) : null}
      {error && <Notice kind="error">{error}</Notice>}
      {!token && <p className="switch-auth"><a href="/signup">Return to create account</a></p>}
    </Card>
  );
}

function clearFragment(): void {
  if (window.location.hash) window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
}
