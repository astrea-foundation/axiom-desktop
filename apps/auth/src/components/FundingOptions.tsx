import type { BillingStatus } from "../api/types";
import { Notice, Spinner } from "./Layout";

export function FundingOptions({ billing, busy, onInvoice }: {
  billing: BillingStatus | null;
  busy: string | null;
  onInvoice(amount: number): void;
}) {
  if (!billing) return <p className="muted compact">Loading payment options…</p>;
  if (billing.payment_account) return <Notice kind="info">
    Manage mainnet Zcash deposits in Axiom Desktop. {billing.payment_account.valuation_enabled
      ? "Confirmed deposits are valued in USD to fund inference credit; funds remain in ZEC."
      : "Deposit credit is currently unavailable; funds remain in ZEC."}
  </Notice>;
  if (!billing.invoices_enabled) return <Notice kind="info">Top-ups are currently unavailable.</Notice>;
  return <>
    <p className="muted compact">Pay through the hosted invoice checkout. Your balance updates after payment confirmation.</p>
    <div className="amount-grid" aria-label="Top-up amounts">
      {[1_000, 2_500, 5_000].map((cents) => <button
        type="button" key={cents} onClick={() => onInvoice(cents)}
        disabled={Boolean(busy) || cents < billing.invoice_min_usd_cents || cents > billing.invoice_max_usd_cents}
      >{busy === `invoice-${cents}` ? <Spinner /> : `$${cents / 100}`}</button>)}
    </div>
  </>;
}
