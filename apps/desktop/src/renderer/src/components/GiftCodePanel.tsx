import { useEffect, useRef, useState } from "react";
import { desktopErrorMessage } from "../signInFlow";
import { usdFromMicrousd } from "../lib/currency";

export function GiftCodePanel({ accountId, connected }: { accountId: string; connected: boolean }) {
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const generation = useRef(0);
  useEffect(() => {
    generation.current += 1;
    pending.current = false;
    setCode(""); setBusy(false); setMessage(null); setError(null);
    return () => { generation.current += 1; };
  }, [accountId, connected]);

  const redeem = async () => {
    const api = window.axiomDesktop?.agent;
    if (!api || !connected || pending.current || !code.trim()) return;
    pending.current = true;
    setBusy(true); setMessage(null); setError(null);
    const current = generation.current;
    try {
      const receipt = await api.redeemGiftCode(code.trim(), accountId);
      if (current !== generation.current) return;
      setCode("");
      const amount = usdFromMicrousd(receipt.creditedMicrousd, 2);
      setMessage(receipt.alreadyRedeemed
        ? `Already redeemed for ${amount}.`
        : `${amount} added to your account.`);
    } catch (failure) {
      if (current !== generation.current) return;
      setError(`${desktopErrorMessage(failure, "Could not confirm redemption.")} You can retry the same code safely.`);
    } finally {
      if (current === generation.current) { pending.current = false; setBusy(false); }
    }
  };

  return <section aria-label="Gift credit" className="mt-5 border-t border-[var(--color-border)] pt-5 text-[13px]">
    <h3 className="font-medium text-[var(--color-text-primary)]">Redeem a gift code</h3>
    <form className="mt-3 flex flex-wrap items-end gap-2" onSubmit={(event) => { event.preventDefault(); void redeem(); }}>
      <label className="min-w-0 flex-1 text-[12px] text-[var(--color-text-secondary)]">Gift code
        <input type="password" name="gift-code" autoComplete="off" spellCheck={false} maxLength={64}
          value={code} disabled={!connected || busy} placeholder="AXG-…"
          onChange={(event) => { setCode(event.target.value); setError(null); setMessage(null); }}
          className="mt-1 block w-full rounded-lg border border-[var(--color-border)] bg-[var(--wash-row)] px-3 py-2 text-[13px] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-text-secondary)] disabled:opacity-40" />
      </label>
      <button type="submit" disabled={!connected || busy || !code.trim()} className="ax-pill ax-pill-button disabled:opacity-40">
        {busy ? "Redeeming…" : "Redeem"}
      </button>
    </form>
    {message ? <p role="status" className="mt-3 text-[var(--color-text-secondary)]">{message}</p> : null}
    {error ? <p role="alert" className="mt-3 text-[var(--color-danger-strong)]">{error}</p> : null}
  </section>;
}
