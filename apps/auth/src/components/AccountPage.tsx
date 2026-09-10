import { useEffect, useRef, useState } from "react";
import { Check, Fingerprint, ShieldCheck, UserRound, Plus, LockKeyhole } from "lucide-react";
import { ConfirmDialog } from "@axiom/ui";
import { MethodIcon } from "./Icons";

import { authApi } from "../api/client";
import type { AccountSummary, BillingStatus, IdentitySummary, LoginMethod, PaymentInvoice } from "../api/types";
import { connectEthereumWallet, signSiweMessage } from "../lib/ethereum";
import { readableError } from "../lib/errors";
import { allowedShieldzPayUrl } from "../lib/payments";
import { registerPasskey } from "../lib/webauthn";
import { Card, Notice, Spinner } from "./Layout";
import { FundingOptions } from "./FundingOptions";

interface AccountPageProps {
  account: AccountSummary;
  onSignedOut(): Promise<void>;
  onRefresh(): Promise<void>;
}

const METHOD_LABELS: Record<LoginMethod, string> = {
  passkey: "Passkey",
  google: "Google",
  password: "Email and password",
  ethereum: "Ethereum wallet",
};

export function AccountPage({ account, onSignedOut, onRefresh }: AccountPageProps) {
  const [identities, setIdentities] = useState<IdentitySummary[]>([]);
  const [identitiesLoaded, setIdentitiesLoaded] = useState(false);
  const [billing, setBilling] = useState<BillingStatus | null>(null);
  const [billingFailed, setBillingFailed] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [confirmLogoutAll, setConfirmLogoutAll] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [notice, setNotice] = useState<{ kind: "success" | "error"; text: string } | null>(null);
  const invoiceRequestIds = useRef(new Map<number, string>());

  useEffect(() => {
    let active = true;
    void Promise.allSettled([authApi.identities(), authApi.billingStatus()]).then(([identityResult, billingResult]) => {
      if (!active) return;
      setIdentitiesLoaded(true);
      if (identityResult.status === "fulfilled") setIdentities(identityResult.value);
      else setNotice({ kind: "error", text: readableError(identityResult.reason) });
      if (billingResult.status === "fulfilled") setBilling(billingResult.value);
      else setBillingFailed(true);
    });
    return () => { active = false; };
  }, []);

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

  async function refreshIdentities() {
    const next = await authApi.identities();
    setIdentities(next);
    setIdentitiesLoaded(true);
    await onRefresh();
  }

  async function linkGoogle() {
    await perform("link-google", async () => {
      window.location.assign(await authApi.googleLinkStart("/account"));
    });
  }

  async function linkPasskey() {
    await perform("link-passkey", async () => {
      const options = await authApi.passkeyRegistrationOptions("link");
      const credential = await registerPasskey(options);
      await authApi.verifyPasskeyRegistration(options.challenge_id, credential, "link");
      await refreshIdentities();
      setNotice({ kind: "success", text: "Passkey linked." });
    });
  }

  async function linkWallet() {
    await perform("link-wallet", async () => {
      const connection = await connectEthereumWallet();
      const challenge = await authApi.siweChallenge(connection.address, connection.chainId, "link");
      const signature = await signSiweMessage(connection, challenge.message);
      await authApi.verifySiwe(challenge.challenge_id, challenge.message, signature, "link");
      await refreshIdentities();
      setNotice({ kind: "success", text: "Ethereum wallet linked." });
    });
  }

  async function removeIdentity(identityId: string) {
    await perform(`remove-${identityId}`, async () => {
      await authApi.removeIdentity(identityId);
      setConfirmRemove(null);
      await refreshIdentities();
      setNotice({ kind: "success", text: "Sign-in method removed." });
    });
  }

  async function createInvoice(cents: number) {
    await perform(`invoice-${cents}`, async () => {
      let requestId = invoiceRequestIds.current.get(cents);
      if (!requestId) {
        requestId = newInvoiceRequestId();
        invoiceRequestIds.current.set(cents, requestId);
      }
      const invoice = await authApi.createInvoice(cents, requestId);
      // A completed API response resolves the create intent. Keep the key only across ambiguous
      // failures so a retry cannot create a duplicate; a later deliberate top-up gets a new key.
      invoiceRequestIds.current.delete(cents);
      goToCheckout(invoice);
    });
  }

  async function logout(all: boolean) {
    await perform(all ? "logout-all" : "logout", async () => {
      if (all) await authApi.logoutAll();
      else await authApi.logout();
      await onSignedOut();
    });
  }

  async function deleteAccount() {
    await perform("delete-account", async () => {
      await authApi.deleteAccount();
      await onSignedOut();
    });
  }

  const name = account.display_name || account.verified_email || "Your Axiom account";
  const initials = name.split(/\s+/).slice(0, 2).map((part) => part[0]).join("").toLocaleUpperCase();
  const confirmation = confirmDelete ? {
    title: "Delete your account?", label: "Delete account",
    detail: "This disables every sign-in method and schedules your account for deletion. Conversations stored on your devices are not erased here.",
    action: deleteAccount,
  } : confirmLogoutAll ? {
    title: "Sign out everywhere?", label: "Sign out everywhere",
    detail: "This revokes your browser, desktop, and automation credentials. You’ll need to sign in again on each device.",
    action: () => logout(true),
  } : confirmRemove ? {
    title: "Remove this sign-in method?", label: "Remove method",
    detail: "You’ll need to use one of your other linked methods the next time you sign in.",
    action: () => removeIdentity(confirmRemove),
  } : null;

  return (
    <div className="account-page">
      <div className="account-heading">
        <div><p className="eyebrow">A SPACE OF YOUR OWN</p><h1>Your account.</h1></div>
        <p>The essentials, all in one place.<br />Make yourself at home.</p>
      </div>
      <div className="account-workspace">
        <nav className="account-nav" aria-label="Account sections">
          <a href="#overview"><UserRound size={16} /> Overview</a>
          <a href="#signin-methods"><Fingerprint size={16} /> Sign-in methods</a>
          <a href="#security"><ShieldCheck size={16} /> Security</a>
          <p className="nav-note">Your conversations live on your device.<br />This space is just for you and your account.</p>
        </nav>
        <div className="account-content">
          {notice && <Notice kind={notice.kind}>{notice.text}</Notice>}
          <Card className="account-profile" >
            <div className="avatar" aria-hidden="true">{initials}</div>
            <div id="overview"><h2>{name}</h2>{account.verified_email && <p>{account.verified_email}</p>}</div>
            <span className="account-status"><Check size={13} /> Signed in</span>
          </Card>
          <div className="account-grid">
            <Card>
              <div className="section-heading" id="signin-methods">
                <div><p className="eyebrow">YOUR WAY IN</p><h2>Sign-in methods</h2></div>
                <span className="count-pill" aria-label={`${identities.length} linked methods`}>{identities.length}</span>
              </div>
              <div className="identity-list">
                {!identitiesLoaded && <div className="center-loading"><Spinner /> Loading methods</div>}
                {identitiesLoaded && identities.length === 0 && <p className="muted compact">No sign-in methods could be loaded. Refresh the page to try again.</p>}
                {identities.map((identity) => <div className="identity-row" key={identity.id}>
                  <span className="method-icon"><MethodIcon method={identity.kind} /></span>
                  <div className="identity-copy"><strong>{METHOD_LABELS[identity.kind]}</strong><span>{identity.label || identity.summary || "Linked to your account"}</span></div>
                  <button type="button" className="quiet-link" disabled={identities.length <= 1 || Boolean(busy)}
                    title={identities.length > 1 ? "Remove method" : "Keep at least one sign-in method"}
                    onClick={() => setConfirmRemove(identity.id)}>Remove</button>
                </div>)}
              </div>
              <div className="link-actions">
                <button type="button" onClick={() => void linkPasskey()} disabled={Boolean(busy)}>{busy === "link-passkey" ? <Spinner /> : <Plus size={12} />} Passkey</button>
                <button type="button" onClick={() => void linkGoogle()} disabled={Boolean(busy)}>{busy === "link-google" ? <Spinner /> : <Plus size={12} />} Google</button>
                <a className="button-link" href="/signup?intent=link"><Plus size={12} /> Email</a>
                <button type="button" onClick={() => void linkWallet()} disabled={Boolean(busy)}>{busy === "link-wallet" ? <Spinner /> : <Plus size={12} />} Wallet</button>
              </div>
              <p className="card-help">Keep at least one method linked. Adding or removing a method may ask you to sign in again.</p>
            </Card>
            <Card>
              <div className="section-heading"><div><p className="eyebrow">ROOM TO THINK</p><h2>Your balance</h2></div></div>
              <div className="balance"><span>{billing ? formatMicroUsd(billing.available_microusd) : "—"}</span><small>available</small></div>
              {billing && <p className="balance-caption"><span>Prepaid inference credit</span><strong>USD</strong></p>}
              {billingFailed ? <Notice kind="error">Your balance couldn’t be loaded. <button className="quiet-link" onClick={() => void perform("reload-balance", async () => { setBilling(await authApi.billingStatus()); setBillingFailed(false); })}>Try again</button></Notice>
                : <FundingOptions billing={billing} busy={busy} onInvoice={(cents) => void createInvoice(cents)} />}
              <p className="funding-caption"><LockKeyhole size={12} /> Private conversations. Pay as you go.</p>
            </Card>
          </div>
          <Card className="security-card">
            <div className="section-heading" id="security"><div><p className="eyebrow">ON YOUR TERMS</p><h2>Account security</h2></div><ShieldCheck size={18} className="muted" /></div>
            <div className="security-row"><div><h3>This browser</h3><p>Sign out here. Your desktop sessions stay connected.</p></div><button className="text-button" onClick={() => void logout(false)} disabled={Boolean(busy)}>Sign out</button></div>
            <div className="security-row"><div><h3>Everywhere else, too</h3><p>Sign out of all browsers, desktops, and connected tools.</p></div><button className="text-button" onClick={() => setConfirmLogoutAll(true)} disabled={Boolean(busy)}>Sign out everywhere</button></div>
            <div className="security-row danger-row"><div><h3>Delete account</h3><p>Close your account and disable every sign-in method.</p></div><button className="danger-button" onClick={() => setConfirmDelete(true)} disabled={Boolean(busy)}>Delete account</button></div>
          </Card>
        </div>
      </div>
      {confirmation && <ConfirmDialog title={confirmation.title} confirmLabel={confirmation.label} busy={Boolean(busy)}
        onCancel={() => { setConfirmRemove(null); setConfirmLogoutAll(false); setConfirmDelete(false); }} onConfirm={() => void confirmation.action()}>
        <p>{confirmation.detail}</p>
        {notice?.kind === "error" && <Notice kind="error">{notice.text}</Notice>}
      </ConfirmDialog>}
    </div>
  );
}

function formatMicroUsd(value: number): string {
  if (!Number.isSafeInteger(value)) return "—";
  return new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" }).format(value / 1_000_000);
}

function newInvoiceRequestId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return `authweb_${Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("")}`;
}

function goToCheckout(invoice: PaymentInvoice): void {
  if (!invoice.pay_url) {
    window.location.assign(`/payments/return?invoice_id=${encodeURIComponent(invoice.id)}`);
    return;
  }
  window.location.assign(allowedShieldzPayUrl(invoice.pay_url));
}
