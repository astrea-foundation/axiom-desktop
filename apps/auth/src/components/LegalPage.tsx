
export function LegalPage({ kind }: { kind: "terms" | "privacy" }) {
  const terms = kind === "terms";
  return (
    <div className="legal-layout"><nav className="legal-nav" aria-label="Policy pages"><p className="eyebrow">THE SMALL PRINT</p><a href="/privacy" aria-current={!terms ? "page" : undefined}>Privacy</a><a href="/terms" aria-current={terms ? "page" : undefined}>Terms of service</a></nav><article className="legal-document"><div className="legal-heading">
      <p className="eyebrow">Legal</p>
      <h1>{terms ? "Terms of service" : "Privacy"}</h1>
      <p className="legal-date">Effective September 4, 2026</p></div>
      {terms ? <TermsShell /> : <PrivacyShell />}
    </article></div>
  );
}

function TermsShell() {
  return (
    <div className="legal-copy">
      <h2>Account access</h2>
      <p>You are responsible for the authenticators and devices linked to your account. Do not approve a desktop request you did not initiate.</p>
      <h2>Prepaid usage</h2>
      <p>Payment returns are informational. Credit is available only after the payment provider confirms settlement and Axiom posts the corresponding ledger entry.</p>
      <h2>Local data</h2>
      <p>Desktop conversation history is stored locally and is not recovered by signing back in. Removing the app or its local account data may be irreversible.</p>
      <h2>Service integrity</h2>
      <p>Do not interfere with authentication, metering, provider attestation, encrypted inference, or access controls.</p>
    </div>
  );
}

function PrivacyShell() {
  return (
    <div className="legal-copy">
      <h2>Account information</h2>
      <p>Axiom processes the identity data needed for the methods you link, session security metadata, invoice status, ledger activity, per-run accounting records, and aggregate inference usage.</p>
      <h2>Conversations</h2>
      <p>This authentication site has no conversation interface or thread storage. Desktop conversations remain on your device. Remote inference content is protected using verified provider E2EE.</p>
      <h2>Authentication providers</h2>
      <p>Google, your passkey provider, an Ethereum wallet, and the payment provider process data under their own policies when you choose those methods.</p>
      <h2>Security</h2>
      <p>Credentials and one-time tokens must not be placed in support messages. Axiom stores password and session verifiers in non-recoverable form where applicable.</p>
    </div>
  );
}
