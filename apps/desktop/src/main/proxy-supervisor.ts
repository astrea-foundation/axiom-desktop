import { randomBytes } from "node:crypto";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { initialProxyState, type ProxyState } from "../shared/proxy";

type Launch = (port: number, accountId: string, token: string) => Promise<ChildProcessWithoutNullStreams>;
const MAX_LINE = 64 * 1024;
const identifier = (value: unknown): value is string => typeof value === "string" && /^[a-zA-Z0-9_.:/-]{1,256}$/.test(value);
const counter = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;

/** Owns only a local process, its local credential, and bounded metadata. */
export class ProxySupervisor {
  private state = initialProxyState();
  private context: string | null = null;
  private generation = 0;
  private child: ChildProcessWithoutNullStreams | null = null;
  private token: string | null = null;
  private disposed = false;
  private pendingStart: Promise<ProxyState> | null = null;
  private stopping: Promise<void> = Promise.resolve();

  constructor(private readonly launch: Launch, private readonly changed: (state: ProxyState) => void, private readonly startupTimeout = 30_000) {}

  snapshot(): ProxyState { return structuredClone(this.state); }
  private publish(patch: Partial<ProxyState>): void {
    this.state = { ...this.state, ...patch, revision: this.state.revision + 1 };
    this.changed(this.snapshot());
  }

  setAccount(accountId: string | null, runtimeId: string | null): void {
    const context = accountId && runtimeId ? JSON.stringify([accountId, runtimeId]) : null;
    if (context === this.context) return;
    this.context = context;
    void this.stop();
    this.publish({ ...initialProxyState(), accountId: context ? accountId : null });
  }

  private requireAccount(accountId: string, runtimeId: string): void {
    if (this.disposed || !this.context || this.context !== JSON.stringify([accountId, runtimeId])) {
      throw new Error("The account or connection changed. Sign in before starting the proxy.");
    }
  }

  copyToken(accountId: string, runtimeId: string): string {
    this.requireAccount(accountId, runtimeId);
    if (this.state.status !== "running" || !this.token) throw new Error("Start the proxy before copying its token.");
    return this.token;
  }

  start(port: number, accountId: string, runtimeId: string): Promise<ProxyState> {
    this.requireAccount(accountId, runtimeId);
    if (!Number.isInteger(port) || port < 1 || port > 65535) return Promise.reject(new Error("Enter a port between 1 and 65535."));
    if (this.pendingStart) return Promise.reject(new Error("The proxy is still starting."));
    const pending = this.startOnce(port, accountId, runtimeId);
    this.pendingStart = pending;
    void pending.finally(() => { if (this.pendingStart === pending) this.pendingStart = null; }).catch(() => {});
    return pending;
  }

  private async startOnce(port: number, accountId: string, runtimeId: string): Promise<ProxyState> {
    const stopped = this.stop();
    const generation = this.generation;
    await stopped;
    if (generation !== this.generation) throw new Error("Proxy start was cancelled.");
    this.requireAccount(accountId, runtimeId);
    const token = randomBytes(32).toString("hex");
    this.publish({ ...initialProxyState(), accountId, port, status: "starting" });
    try {
      const child = await this.launch(port, accountId, token);
      if (generation !== this.generation || this.disposed) {
        await this.terminate(child);
        throw new Error("Proxy start was cancelled.");
      }
      this.requireAccount(accountId, runtimeId);
      this.child = child;
      this.token = token;
      child.stdin.on("error", () => {}); // EOF/EPIPE during shutdown is expected.
      await new Promise<void>((resolve, reject) => {
        let ready = false;
        let stdout = "";
        let stderr = "";
        const owned = () => this.child === child && generation === this.generation;
        const timer = setTimeout(() => fail("The proxy did not become ready. Check your account and try again."), this.startupTimeout);
        const fail = (message: string) => {
          clearTimeout(timer);
          if (owned()) {
            this.token = null;
            this.publish({ status: "failed", baseUrl: null, error: message });
            child.stdin.end();
            child.kill();
          }
          reject(new Error(message));
        };
        child.once("error", () => fail("Could not launch the native proxy. Rebuild or reinstall Axiom."));
        child.once("close", () => {
          clearTimeout(timer);
          if (owned()) {
            this.child = null;
            this.token = null;
            this.publish({ status: "failed", baseUrl: null, error: "The proxy stopped unexpectedly. Check your account and whether the port is already in use." });
          }
          if (!ready) reject(new Error("The proxy could not start. Check your account and whether the port is already in use."));
        });
        child.stdout.setEncoding("utf8");
        child.stderr.setEncoding("utf8");
        child.stdout.on("data", (chunk: string) => {
          if (!owned()) return;
          stdout += chunk;
          if (Buffer.byteLength(stdout) > MAX_LINE) return fail("The native proxy returned invalid startup data.");
          const newline = stdout.indexOf("\n");
          if (newline < 0) return;
          try {
            const data = JSON.parse(stdout.slice(0, newline));
            stdout = stdout.slice(newline + 1);
            if (ready || data.event !== "ready" || data.address !== `127.0.0.1:${port}`
              || data.openai_base_url !== `http://127.0.0.1:${port}/v1` || data.security !== "attestation_per_request"
              || stdout.trim()) throw new Error();
            ready = true;
            clearTimeout(timer);
            this.publish({ status: "running", baseUrl: data.openai_base_url, error: null });
            resolve();
          } catch { fail("The native proxy returned invalid startup data. Update Axiom and try again."); }
        });
        child.stderr.on("data", (chunk: string) => {
          if (!owned()) return;
          stderr += chunk;
          if (Buffer.byteLength(stderr) > MAX_LINE) return fail("The native proxy exceeded its status output limit.");
          let newline: number;
          while ((newline = stderr.indexOf("\n")) >= 0) {
            const line = stderr.slice(0, newline);
            stderr = stderr.slice(newline + 1);
            // Never forward raw diagnostics or unknown fields to the renderer.
            try { this.acceptMetadata(JSON.parse(line)); } catch { /* non-JSON diagnostic */ }
          }
        });
      });
      this.requireAccount(accountId, runtimeId);
      if (generation !== this.generation) throw new Error("Proxy start was cancelled.");
      return this.snapshot();
    } catch (error) {
      if (generation === this.generation) {
        const stopped = this.stop();
        const stoppedGeneration = this.generation;
        await stopped;
        if (stoppedGeneration === this.generation) this.publish({ status: "failed", error: "Could not start the proxy. Check your account, update the native app, or try a different port." });
      }
      throw error instanceof Error ? error : new Error("Could not start the proxy.");
    }
  }

  private acceptMetadata(data: Record<string, unknown>): void {
    if (!data || typeof data !== "object") return;
    if (data.event === "security_verified" && identifier(data.request_id) && identifier(data.model_id)
      && identifier(data.provider_id) && identifier(data.e2ee_protocol) && counter(data.verified_at_unix_seconds)
      && typeof data.model_key_fingerprint === "string" && /^[a-fA-F0-9]{32,128}$/.test(data.model_key_fingerprint)) {
      this.publish({ evidence: { requestId: data.request_id, modelId: data.model_id, providerId: data.provider_id,
        protocol: data.e2ee_protocol, verifiedAt: data.verified_at_unix_seconds, fingerprint: data.model_key_fingerprint, responseVerified: false } });
    } else if (data.event === "request_terminal" && identifier(data.request_id) && data.terminal === "completed" && counter(data.total_tokens)) {
      const total = this.state.totalTokens + data.total_tokens;
      const evidence = this.state.evidence;
      this.publish({ completedRequests: this.state.completedRequests + 1,
        totalTokens: Number.isSafeInteger(total) ? total : this.state.totalTokens,
        evidence: evidence && evidence.requestId === data.request_id ? { ...evidence, responseVerified: true } : evidence });
    } else if ((data.event === "request_failed" || data.event === "security_failed") && identifier(data.failure_kind)) {
      this.publish({ errors: [{ kind: data.failure_kind, at: Date.now() }, ...this.state.errors].slice(0, 20) });
    }
  }

  private terminate(child: ChildProcessWithoutNullStreams): Promise<void> {
    if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
    return new Promise((resolve) => {
      const timer = setTimeout(() => child.kill("SIGKILL"), 3_000);
      child.once("close", () => { clearTimeout(timer); resolve(); });
      child.stdin.on("error", () => {});
      child.stdin.end();
    });
  }

  async stop(): Promise<ProxyState> {
    this.generation++;
    this.token = null;
    const child = this.child;
    this.child = null;
    if (child) {
      this.publish({ status: "stopping", baseUrl: null });
      this.stopping = Promise.all([this.stopping, this.terminate(child)]).then(() => {});
    }
    const generation = this.generation;
    await this.stopping;
    if (generation === this.generation) this.publish({ status: "stopped", baseUrl: null, error: null });
    return this.snapshot();
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    await this.stop();
    await this.pendingStart?.catch(() => {});
  }
}
