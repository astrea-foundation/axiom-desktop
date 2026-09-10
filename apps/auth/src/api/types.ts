export type LoginMethod = "passkey" | "google" | "password" | "ethereum";
export type AuthIntent = "signin" | "signup" | "link";

export interface AccountSummary {
  id: string;
  display_name: string | null;
  avatar_url: string | null;
  verified_email: string | null;
  linked_methods: LoginMethod[];
}

export interface BrowserSessionSummary {
  id: string;
  kind: "browser" | "native";
  expires_at: string;
}

export interface SessionResponse {
  authenticated: boolean;
  account?: AccountSummary;
  session?: BrowserSessionSummary;
  csrf_token?: string;
}

export interface EmailLoginChallenge {
  authenticated: false;
  challenge_id: string;
  expires_at: string;
  resend_after_seconds: number;
}

export interface IdentitySummary {
  id: string;
  kind: LoginMethod;
  label: string | null;
  summary: string | null;
  created_at: string;
  last_used_at: string | null;
}

export interface PublicKeyDescriptorJson {
  id: string;
  type: "public-key";
  transports?: AuthenticatorTransport[];
}

export interface PasskeyRequestOptions {
  challenge_id: string;
  public_key: {
    challenge: string;
    timeout?: number;
    rpId?: string;
    allowCredentials?: PublicKeyDescriptorJson[];
    userVerification?: UserVerificationRequirement;
    extensions?: AuthenticationExtensionsClientInputs;
  };
}

export interface PasskeyCreationOptions {
  challenge_id: string;
  public_key: {
    challenge: string;
    rp: PublicKeyCredentialRpEntity;
    user: Omit<PublicKeyCredentialUserEntity, "id"> & { id: string };
    pubKeyCredParams: PublicKeyCredentialParameters[];
    timeout?: number;
    excludeCredentials?: PublicKeyDescriptorJson[];
    authenticatorSelection?: AuthenticatorSelectionCriteria;
    attestation?: AttestationConveyancePreference;
    extensions?: AuthenticationExtensionsClientInputs;
  };
}

export interface SiweChallenge {
  challenge_id: string;
  message: string;
  expires_at: string;
}

export interface NativeAuthorizationSummary {
  id: string;
  status: "pending" | "approved" | "consumed" | "expired" | "cancelled";
  user_code: string;
  client_name: string;
  client_platform?: string | null;
  method_hint?: LoginMethod | null;
  callback_uri?: string;
  expires_at: string;
}

export interface NativeApprovalResult {
  status: "approved";
  callback_uri?: string;
}

export interface BillingStatus {
  available_microusd: number;
  posted_microusd: number;
  held_microusd: number;
  ledger_sequence: number;
  currency: "microUSD";
  invoices_enabled: boolean;
  invoice_min_usd_cents: number;
  invoice_max_usd_cents: number;
  settlement_asset: "USDC" | null;
  payment_account?: {
    network: "mainnet";
    asset: "ZEC";
    conversion_status: "none";
    valuation_enabled: boolean;
  } | null;
}

export interface PaymentInvoice {
  id: string;
  amount_usd_cents: number;
  state: "creating" | "pending" | "paid" | "expired" | "failed" | "review";
  pay_url?: string;
  expires_at: string | null;
  paid_at: string | null;
}

export interface PublicResult {
  ok: boolean;
  message?: string;
}
