import { useEffect, useState } from "react";
import { Check, CheckCheck, Monitor, ArrowRight, Clock3 } from "lucide-react";
import { authApi } from "../api/client";
import type { NativeAuthorizationSummary } from "../api/types";
import { readableError } from "../lib/errors";
import { nativeLoopbackCallback } from "../lib/navigation";
import { Card, Notice, Spinner, PageHeading } from "./Layout";

export function NativeApprovalPage({ authorizationId, authorizationState }: { authorizationId: string | null; authorizationState: string | null }) {
  const [authorization, setAuthorization] = useState<NativeAuthorizationSummary | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    if (!authorizationId || !authorizationState) { setNotice("This desktop authorization link is incomplete or invalid."); setLoading(false); return; }
    void authApi.nativeAuthorization(authorizationId, authorizationState)
      .then((value) => { if (active) setAuthorization(value); })
      .catch((error) => { if (active) setNotice(readableError(error)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [authorizationId, authorizationState]);
  async function approve() {
    if (!authorizationId || !authorizationState || busy) return;
    setBusy(true); setNotice(null);
    try {
      const result = await authApi.approveNativeAuthorization(authorizationId, authorizationState);
      const callback = nativeLoopbackCallback(result.callback_uri, authorizationId, authorizationState);
      if (callback) window.location.replace(callback);
      else setAuthorization((current) => current ? { ...current, status: "approved" } : current);
    } catch (error) { setNotice(readableError(error)); }
    finally { setBusy(false); }
  }
  const approved = authorization?.status === "approved" || authorization?.status === "consumed";
  const terminal = authorization && !approved && authorization.status !== "pending";
  return <Card className="native-card">
    <div className={`status-symbol ${approved ? "success" : ""}`}>{approved ? <CheckCheck size={23} /> : terminal ? <Clock3 size={23} /> : <Monitor size={23} />}</div>
    <PageHeading eyebrow="DESKTOP CONNECTION" title={approved ? "You’re all set." : terminal ? "Let’s start again." : "Meet your desktop."}>
      {approved ? "Your approval is complete. Return to Axiom to finish connecting." : terminal ? "This request is no longer active. Start a new sign-in from Axiom Desktop." : "Make sure this code matches the one in Axiom Desktop before connecting."}
    </PageHeading>
    {loading && <div className="center-loading"><Spinner label="Loading authorization" /> Loading your request</div>}
    {notice && <Notice kind="error">{notice}</Notice>}
    {authorization && !terminal && !approved && <>
      <div className="native-device"><Monitor size={23} /><div><strong>{authorization.client_name}</strong><small>{authorization.client_platform || "Axiom Desktop"}</small></div></div>
      <div className="verification-code"><span>YOUR VERIFICATION CODE</span><strong className="user-code">{authorization.user_code}</strong></div>
      <div className="scope-box"><strong>This device will be able to</strong><ul>
        <li><Check size={14} /> View your account profile</li><li><Check size={14} /> View your usage balance</li><li><Check size={14} /> Run encrypted model requests</li>
      </ul></div>
      <button type="button" className="action-button" onClick={() => void approve()} disabled={busy}>{busy ? <Spinner /> : null} Connect desktop <ArrowRight size={15} /></button>
    </>}
    {approved && <Notice kind="success">Approved. You can close this page and return to Axiom Desktop.</Notice>}
    {terminal && <Notice kind="info">This request is {authorization.status}.</Notice>}
    <p className="fine-print">Only connect a device you’re signing in to.<br />Your conversations stay on that device.</p>
  </Card>;
}
