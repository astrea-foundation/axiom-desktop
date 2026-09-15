import type { BillingInvoice, BillingInvoiceResponse, ClientState } from "@axiom/axiom-acp-client";
import type { BillingInvoiceRequestTracker } from "./billingInvoiceRequest";

export interface BillingCheckoutApi {
  getState(): Promise<Pick<ClientState, "connected" | "account" | "invoice">>;
  createBillingInvoice(amountUsdCents: number, clientRequestId: string, accountId: string): Promise<BillingInvoiceResponse>;
  billingInvoiceStatus(invoiceId: string): Promise<BillingInvoiceResponse>;
  openBillingPayment(paymentUrl: string, accountId: string): Promise<void>;
}

export function awaitingPayment(invoice: BillingInvoice): boolean {
  return invoice.state === "creating" || invoice.state === "pending";
}

/** One explicit Top up gesture creates/resumes checkout and opens it once. */
export class BillingCheckoutFlow {
  private flight: { accountId: string; amount: number; task: Promise<void> } | null = null;

  constructor(private readonly requests: BillingInvoiceRequestTracker) {}

  topUp(accountId: string, amount: number, api: BillingCheckoutApi): Promise<void> {
    if (this.flight?.accountId === accountId) {
      return this.flight.amount === amount
        ? this.flight.task
        : Promise.reject(new Error("Another top-up is still opening. Please wait."));
    }
    const task = this.perform(accountId, amount, api);
    this.flight = { accountId, amount, task };
    const clear = () => { if (this.flight?.task === task) this.flight = null; };
    void task.then(clear, clear);
    return task;
  }

  private async perform(accountId: string, amount: number, api: BillingCheckoutApi): Promise<void> {
    const state = await api.getState();
    if (!state.connected || state.account?.state !== "valid" || state.account.account?.id !== accountId) {
      throw new Error("Sign in to top up your balance.");
    }
    const pending = this.requests.snapshot();
    const retrying = pending?.accountId === accountId && pending.amountUsdCents === amount;
    let invoice: BillingInvoice;
    if (!retrying && state.invoice?.amountUsdCents === amount && awaitingPayment(state.invoice)) {
      // Reopen the existing invoice, but check its current server state first:
      // the customer may have paid while the browser was in the foreground.
      invoice = (await api.billingInvoiceStatus(state.invoice.id)).invoice;
      await this.openIfPayable(invoice, accountId, api);
    } else {
      invoice = await this.requests.submit(accountId, amount, async (clientRequestId) => {
        const created = (await api.createBillingInvoice(amount, clientRequestId, accountId)).invoice;
        // Keep the durable retry key if browser launch fails, including across
        // renderer restarts. Retrying must not create a second invoice.
        await this.openIfPayable(created, accountId, api);
        return created;
      });
    }
    // Terminal responses are definitive and clear any creation retry key. A
    // paid retry must never open checkout or silently start a second payment.
    if (invoice.state === "expired" || invoice.state === "failed") {
      throw new Error("This top-up has ended. Press Top up to start a new one.");
    }
    if (invoice.state === "review") {
      throw new Error("This payment is under review. Please wait before paying again.");
    }
  }

  private async openIfPayable(invoice: BillingInvoice, accountId: string, api: BillingCheckoutApi): Promise<void> {
    if (!awaitingPayment(invoice)) return;
    if (!invoice.payUrl) throw new Error("Checkout is still being prepared. Please try again.");
    // Electron main checks the current account, invoice state/expiry, and
    // Shieldz URL immediately before using the cross-platform browser opener.
    try {
      await api.openBillingPayment(invoice.payUrl, accountId);
    } catch {
      throw new Error("Could not open checkout. Use Reopen checkout to try again.");
    }
  }
}

type MonitorApi = {
  billingInvoiceStatus(invoiceId: string): Promise<BillingInvoiceResponse>;
  billingStatus(): Promise<unknown>;
};
type Schedule = (callback: () => void, delay: number) => () => void;

/** Runs outside Settings, never opens a browser, and retries balance refresh. */
export function monitorBillingInvoice(
  invoice: BillingInvoice,
  api: MonitorApi,
  schedule: Schedule = (callback, delay) => {
    const timer = setTimeout(callback, delay);
    return () => clearTimeout(timer);
  },
): { refresh(): Promise<void>; stop(): Promise<void> } {
  let state = invoice.state;
  let stopped = !awaitingPayment(invoice) && state !== "paid";
  let inFlight: Promise<void> | null = null;
  let cancelTimer: (() => void) | null = null;
  const enqueue = (delay: number) => {
    cancelTimer = schedule(() => { void refresh(); }, delay);
  };
  const poll = async () => {
    cancelTimer?.();
    cancelTimer = null;
    try {
      if (state !== "paid") state = (await api.billingInvoiceStatus(invoice.id)).invoice.state;
      if (stopped) return;
      if (state === "paid") await api.billingStatus();
      if (state !== "creating" && state !== "pending") stopped = true;
    } catch {
      // Network failure is not payment success. Retry the last verified state;
      // native account-context checks reject responses after sign-out/switch.
    } finally {
      if (!stopped) enqueue(5_000);
    }
  };
  const refresh = () => {
    if (stopped) return Promise.resolve();
    if (inFlight) return inFlight;
    const task = poll();
    inFlight = task;
    const clear = () => { if (inFlight === task) inFlight = null; };
    void task.then(clear, clear);
    return task;
  };
  if (!stopped) enqueue(state === "paid" ? 0 : 5_000);
  return {
    refresh,
    stop: () => {
      stopped = true;
      cancelTimer?.();
      // Foreground checkout can drain the last status RPC before creating a
      // different invoice, preventing a late poll from replacing its state.
      return inFlight ?? Promise.resolve();
    },
  };
}
