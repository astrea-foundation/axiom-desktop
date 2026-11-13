const STORAGE_KEY = "axiom.desktop.pending-billing-invoice.v1";
const MAX_ACCOUNT_ID_BYTES = 128;
const MAX_REQUEST_ID_BYTES = 128;
const MAX_INVOICE_USD_CENTS = 10_000_000;

export interface PendingInvoiceRequest {
  accountId: string;
  amountUsdCents: number;
  clientRequestId: string;
}

export interface InvoiceRequestStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function validPending(value: unknown): value is PendingInvoiceRequest {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<PendingInvoiceRequest>;
  return typeof candidate.accountId === "string"
    && candidate.accountId.length > 0
    && candidate.accountId.length <= MAX_ACCOUNT_ID_BYTES
    && /^[A-Za-z0-9_-]+$/.test(candidate.accountId)
    && Number.isSafeInteger(candidate.amountUsdCents)
    && (candidate.amountUsdCents ?? 0) > 0
    && (candidate.amountUsdCents ?? 0) <= MAX_INVOICE_USD_CENTS
    && typeof candidate.clientRequestId === "string"
    && candidate.clientRequestId.length > 0
    && candidate.clientRequestId.length <= MAX_REQUEST_ID_BYTES
    && /^[A-Za-z0-9_.:-]+$/.test(candidate.clientRequestId);
}

function sameRequest(left: PendingInvoiceRequest, right: PendingInvoiceRequest): boolean {
  return left.accountId === right.accountId
    && left.amountUsdCents === right.amountUsdCents
    && left.clientRequestId === right.clientRequestId;
}

/**
 * Owns the Desktop invoice idempotency key.
 *
 * An RPC error cannot prove whether the backend committed an invoice, so the
 * key remains pending across timeout, cancellation, disconnect, and generic
 * errors. A successful send callback clears it; checkout callers include the
 * browser handoff so a launch failure also preserves the key. A different account
 * or amount is an explicit abandonment boundary and receives a new key.
 */
export class BillingInvoiceRequestTracker {
  private pending: PendingInvoiceRequest | null = null;
  private loaded = false;
  private storageFailure: Error | null = null;

  constructor(
    private readonly createId: () => string,
    private readonly storage: InvoiceRequestStorage,
  ) {}

  async submit<T>(
    accountId: string,
    amountUsdCents: number,
    send: (clientRequestId: string) => Promise<T>,
  ): Promise<T> {
    const request = this.requestFor(accountId, amountUsdCents);
    const result = await send(request.clientRequestId);
    this.complete(request);
    return result;
  }

  /**
   * Reconcile a definitive account identity. `undefined` means startup or a
   * transient outage where clearing a durable retry key would be ambiguous.
   */
  reconcileAccount(accountId: string | null | undefined): void {
    if (accountId === undefined) return;
    this.load();
    if (this.storageFailure) return;
    if (accountId === null || this.pending?.accountId !== accountId) this.clear();
  }

  snapshot(): PendingInvoiceRequest | null {
    this.load();
    return this.pending ? { ...this.pending } : null;
  }

  private requestFor(accountId: string, amountUsdCents: number): PendingInvoiceRequest {
    this.load();
    if (this.storageFailure) throw this.storageFailure;
    if (
      this.pending?.accountId === accountId
      && this.pending.amountUsdCents === amountUsdCents
    ) {
      return { ...this.pending };
    }
    const candidate = { accountId, amountUsdCents, clientRequestId: this.createId() };
    if (!validPending(candidate)) throw new Error("invoice request context is invalid");
    this.pending = candidate;
    this.persist();
    return { ...candidate };
  }

  private complete(request: PendingInvoiceRequest): void {
    this.load();
    if (this.pending && sameRequest(this.pending, request)) this.clear();
  }

  private load(): void {
    if (this.loaded) return;
    this.loaded = true;
    let encoded: string | null;
    try {
      encoded = this.storage.getItem(STORAGE_KEY);
    } catch {
      this.storageFailure = unavailableStorage();
      return;
    }
    if (encoded === null) return;
    if (encoded.length > 1_024) {
      this.discardInvalidStoredValue();
      return;
    }
    try {
      const value: unknown = JSON.parse(encoded);
      if (validPending(value)) {
        this.pending = value;
      } else {
        this.discardInvalidStoredValue();
      }
    } catch {
      this.discardInvalidStoredValue();
    }
  }

  private persist(): void {
    if (!this.pending) return;
    try {
      this.storage.setItem(STORAGE_KEY, JSON.stringify(this.pending));
    } catch {
      this.pending = null;
      this.storageFailure = unavailableStorage();
      throw this.storageFailure;
    }
  }

  private clear(): void {
    try {
      this.storage.removeItem(STORAGE_KEY);
      this.pending = null;
    } catch {
      // Keep the exact request in memory. A later retry remains idempotent even
      // when durable cleanup is temporarily unavailable.
    }
  }

  private discardInvalidStoredValue(): void {
    try {
      this.storage.removeItem(STORAGE_KEY);
      this.pending = null;
    } catch {
      this.storageFailure = unavailableStorage();
    }
  }
}

function unavailableStorage(): Error {
  return new Error("Invoice retry storage is unavailable; no invoice was sent");
}

export function browserInvoiceRequestStorage(): InvoiceRequestStorage {
  // Resolve localStorage lazily: a denied getter becomes a bounded top-up
  // error rather than crashing the renderer during application startup.
  return {
    getItem: (key) => window.localStorage.getItem(key),
    setItem: (key, value) => window.localStorage.setItem(key, value),
    removeItem: (key) => window.localStorage.removeItem(key),
  };
}
