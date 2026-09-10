import { useEffect, useState } from "react";

import { authApi } from "../api/client";
import type { PaymentInvoice } from "../api/types";
import { readableError } from "../lib/errors";
import { Check, Clock3, CreditCard, AlertCircle } from "lucide-react";
import { Card, Notice, Spinner } from "./Layout";

export function PaymentStatusPage({ invoiceId }: { invoiceId: string | null }) {
  const [invoice, setInvoice] = useState<PaymentInvoice | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function refresh() {
    if (!invoiceId || loading) return;
    setLoading(true);
    setError(null);
    try {
      setInvoice(await authApi.invoice(invoiceId));
    } catch (reason) {
      setError(readableError(reason));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => { void refresh(); }, [invoiceId]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <Card className="status-card">
      <div className={`status-symbol ${invoice?.state === "paid" ? "success" : ""}`}>{invoice?.state === "paid" ? <Check size={23} /> : invoice?.state === "review" ? <AlertCircle size={23} /> : invoice?.state === "pending" ? <Clock3 size={23} /> : <CreditCard size={23} />}</div>
      <p className="eyebrow">YOUR PAYMENT</p>
      <h1>{invoice?.state === "paid" ? "Room for more." : "A little patience."}</h1>
      <p className="lead-small">The latest on your account top-up.</p>
      {!invoiceId && <Notice kind="error">No invoice was specified.</Notice>}
      {loading && !invoice && <div className="center-loading"><Spinner /></div>}
      {error && <Notice kind="error">{error}</Notice>}
      {invoice && (
        <div className="payment-result">
          <h2>{invoiceStateTitle(invoice.state)}</h2>
          <p>{invoiceStateCopy(invoice.state)}</p>
          <dl>
            <div><dt>Amount</dt><dd>${(invoice.amount_usd_cents / 100).toFixed(2)}</dd></div>
            <div><dt>Status</dt><dd>{invoice.state}</dd></div>
          </dl>
        </div>
      )}
      <div className="status-actions">
        <button type="button" className="text-button" onClick={() => void refresh()} disabled={!invoiceId || loading}>
          {loading && <Spinner />} Refresh status
        </button>
        <a className="button-link" href="/account">Back to account</a>
      </div>
      <p className="fine-print">This page is informational. Your balance changes only after Axiom authenticates and reconciles the paid invoice server-side.</p>
    </Card>
  );
}

function invoiceStateTitle(state: PaymentInvoice["state"]): string {
  if (state === "paid") return "Payment confirmed";
  if (state === "creating" || state === "pending") return "Waiting for confirmation";
  if (state === "review") return "Payment under review";
  return `Invoice ${state}`;
}

function invoiceStateCopy(state: PaymentInvoice["state"]): string {
  if (state === "paid") return "The credit has been posted to your Axiom account.";
  if (state === "creating" || state === "pending") return "Settlement can take a moment. You may safely return later.";
  if (state === "review") return "No duplicate credit will be posted while reconciliation completes.";
  return "This invoice did not add credit. You can create a new invoice from your account page.";
}
