import { ArrowLeft, CloudOff, Compass } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import { authApi } from "./api/client";
import type { SessionResponse } from "./api/types";
import { AccountPage } from "./components/AccountPage";
import { AuthPanel } from "./components/AuthPanel";
import { Card, Layout, Notice, Spinner } from "./components/Layout";
import { LegalPage } from "./components/LegalPage";
import { NativeApprovalPage } from "./components/NativeApprovalPage";
import { PaymentStatusPage } from "./components/PaymentStatusPage";
import { RecoverPage, ResetPasswordPage, VerifyEmailPage } from "./components/RecoveryPages";
import { routeForPath, tokenFromFragment, watchTokenFragment } from "./lib/navigation";
import { safeReturnPath } from "./api/client";

export function App() {
  const route = routeForPath(window.location.pathname);
  const params = useMemo(() => new URLSearchParams(window.location.search), []);
  const [secretToken, setSecretToken] = useState(() => tokenFromFragment(window.location.hash));
  const [session, setSession] = useState<SessionResponse | null>(null);
  const [sessionFailed, setSessionFailed] = useState(false);

  useEffect(() => watchTokenFragment(window, setSecretToken), []);

  const returnTo = safeReturnPath(
    params.get("return_to") ??
      (route === "native" || route === "payment"
        ? `${window.location.pathname}${window.location.search}`
        : "/account"),
  );

  async function refreshSession() {
    const next = await authApi.session();
    setSession(next);
    setSessionFailed(false);
    return next;
  }

  useEffect(() => {
    let active = true;
    if (route === "terms" || route === "privacy" || route === "reset" || route === "verify" || route === "not-found") return;
    void authApi.session()
      .then((next) => { if (active) setSession(next); })
      .catch(() => { if (active) setSessionFailed(true); });
    return () => { active = false; };
  }, [route]);

  async function authenticated() {
    const next = await refreshSession();
    if (!next.authenticated) throw new Error("Authentication did not complete");
    window.location.replace(returnTo);
  }

  async function signedOut() {
    setSession({ authenticated: false });
    window.location.replace("/signin");
  }

  if (route === "terms" || route === "privacy") {
    return <Layout wide><LegalPage kind={route} /></Layout>;
  }
  // Remount when an email link replaces the capability in the same document;
  // never carry a password, success state, or pending result to a new token.
  if (route === "reset") return <Layout><ResetPasswordPage key={secretToken} token={secretToken} /></Layout>;
  if (route === "verify") return <Layout><VerifyEmailPage key={secretToken} token={secretToken} /></Layout>;
  if (route === "recover") return <Layout><RecoverPage /></Layout>;
  if (route === "not-found") {
    return <Layout><Card className="status-card"><div className="status-symbol"><Compass size={23} /></div><p className="eyebrow">404 · A WRONG TURN</p><h1>A little off course.</h1><p className="lead-small">We couldn’t find that page. Let’s get you back to your space.</p><a className="action-button full-width" href="/account"><ArrowLeft size={14} /> Back to your account</a></Card></Layout>;
  }

  if (sessionFailed) {
    return (
      <Layout>
        <Card className="status-card">
          <div className="status-symbol"><CloudOff size={23} /></div>
          <p className="eyebrow">CONNECTION</p>
          <h1>A brief interruption.</h1>
          <p className="lead-small">We couldn’t reach your account. Give it a moment, then try again.</p>
          <Notice kind="error">Axiom could not load your secure browser session.</Notice>
          <div className="status-actions"><button type="button" className="action-button" onClick={() => window.location.reload()}>Try again</button></div>
        </Card>
      </Layout>
    );
  }

  if (!session) {
    return <Layout><Card className="loading-card"><Spinner label="Loading account" /><span>Loading your account…</span></Card></Layout>;
  }

  const account = authApi.accountFromSession(session);
  if (!account) {
    return (
      <Layout>
        <Card>
          <p className="eyebrow">Axiom account</p>
          <h1>{route === "signup" ? "Make room for you." : route === "native" ? "Sign in to approve" : "Welcome back."}</h1>
          <p className="lead-small">
            {route === "native"
              ? "Sign in to connect your desktop. You’ll review the request next."
              : route === "signup"
                ? "One account for private conversations, code, and a little more possibility."
                : "Sign in to your little corner of Axiom."}
          </p>
          {params.get("auth") === "cancelled" && <Notice kind="info">Sign-in was cancelled. Nothing was changed.</Notice>}
          {params.get("auth") === "failed" && <Notice kind="error">Sign-in could not be completed. Please try again.</Notice>}
          <AuthPanel mode={route === "signup" ? "signup" : "signin"} returnTo={returnTo} onAuthenticated={authenticated} />
        </Card>
      </Layout>
    );
  }

  if (route === "native") {
    return (
      <Layout>
        <NativeApprovalPage
          authorizationId={params.get("authorization_id")}
          authorizationState={params.get("state")}
        />
      </Layout>
    );
  }

  if (route === "payment") {
    return <Layout><PaymentStatusPage invoiceId={params.get("invoice_id")} /></Layout>;
  }

  if (route === "signup" && params.get("intent") === "link") {
    return (
      <Layout>
        <Card>
          <p className="eyebrow">Link a method</p>
          <h1>Another way in.</h1>
          <p className="lead-small">Verify a new email and password without changing your account identity.</p>
          <AuthPanel mode="link-password" returnTo="/account" onAuthenticated={async () => { await refreshSession(); }} />
          <p className="switch-auth"><a href="/account">Cancel and return to account</a></p>
        </Card>
      </Layout>
    );
  }

  return (
    <Layout wide>
      <AccountPage account={account} onSignedOut={signedOut} onRefresh={async () => { await refreshSession(); }} />
    </Layout>
  );
}
