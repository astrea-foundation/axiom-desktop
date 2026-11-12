export interface ProxyEvidence {
  requestId: string;
  modelId: string;
  providerId: string;
  protocol: string;
  verifiedAt: number;
  fingerprint: string;
  responseVerified: boolean;
}

export interface ProxyState {
  revision: number;
  status: "stopped" | "starting" | "running" | "stopping" | "failed";
  accountId: string | null;
  port: number;
  baseUrl: string | null;
  completedRequests: number;
  totalTokens: number;
  errors: { kind: string; at: number }[];
  evidence: ProxyEvidence | null;
  error: string | null;
}

export interface ProxyApi {
  getState(): Promise<ProxyState>;
  start(port: number, accountId: string, runtimeId: string): Promise<ProxyState>;
  stop(): Promise<ProxyState>;
  copyToken(accountId: string, runtimeId: string): Promise<void>;
  onState(callback: (state: ProxyState) => void): () => void;
}

export const initialProxyState = (): ProxyState => ({
  revision: 0, status: "stopped", accountId: null, port: 8484, baseUrl: null,
  completedRequests: 0, totalTokens: 0, errors: [], evidence: null, error: null,
});
