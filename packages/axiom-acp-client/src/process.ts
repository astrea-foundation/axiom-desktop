import { EventEmitter, once } from "node:events";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { AcpError, ProtocolError, SidecarExitedError } from "./errors.js";
import type { JsonObject, RpcNotification, RpcRequest } from "./types.js";

const MAX_FRAME_BYTES = 4 * 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES = 64 * 1024;

type Pending = {
  resolve: (value: unknown) => void;
  reject: (reason: unknown) => void;
  timer: NodeJS.Timeout | null;
  abort: { signal: AbortSignal; listener: () => void } | null;
};

export type IncomingRequest = {
  request: RpcRequest;
  respond: (result: unknown) => void;
  reject: (code: number, message: string) => void;
};

export interface SidecarProcessOptions {
  command: string;
  args?: string[];
  cwd?: string;
  env?: NodeJS.ProcessEnv;
}

export class AcpProcess extends EventEmitter {
  readonly child: ChildProcessWithoutNullStreams;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private diagnostics = "";
  private closed = false;
  private stdoutRemainder = Buffer.alloc(0);

  constructor(options: SidecarProcessOptions) {
    super();
    this.child = spawn(options.command, options.args ?? ["acp"], {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
      shell: false,
    });

    this.child.stdout.on("data", (chunk: Buffer) => this.acceptChunk(chunk));
    this.child.stderr.on("data", (chunk: Buffer) => {
      this.diagnostics = `${this.diagnostics}${chunk.toString("utf8")}`.slice(
        -MAX_DIAGNOSTIC_BYTES,
      );
      this.emit("diagnostic", this.diagnostics);
    });
    this.child.once("error", (error) => this.failAll(error));
    this.child.once("exit", (code, signal) => {
      this.closed = true;
      const error = new SidecarExitedError(code, signal, this.diagnostics);
      this.failAll(error);
      this.emit("exit", error);
    });
  }

  async request<T>(
    method: string,
    params: unknown,
    timeoutMs = 120_000,
    signal?: AbortSignal,
  ): Promise<T> {
    if (this.closed) throw new ProtocolError("AxiomCLI is not running");
    if (signal?.aborted) throw new ProtocolError(`${method} was cancelled`);
    const id = this.nextId++;
    const response = new Promise<T>((resolve, reject) => {
      const timer =
        timeoutMs > 0
          ? setTimeout(() => {
              const current = this.pending.get(id);
              if (current?.abort) {
                current.abort.signal.removeEventListener("abort", current.abort.listener);
              }
              this.pending.delete(id);
              void this.notify("$/cancel_request", { requestId: id }).catch(() => undefined);
              reject(new ProtocolError(`${method} timed out`));
            }, timeoutMs)
          : null;
      const listener = () => {
        void this.notify("$/cancel_request", { requestId: id }).catch(() => undefined);
      };
      const abort = signal ? { signal, listener } : null;
      signal?.addEventListener("abort", listener, { once: true });
      this.pending.set(id, {
        resolve: resolve as (value: unknown) => void,
        reject,
        timer,
        abort,
      });
    });
    try {
      await this.write({ jsonrpc: "2.0", id, method, params });
    } catch (error) {
      const pending = this.pending.get(id);
      if (pending?.timer) clearTimeout(pending.timer);
      if (pending?.abort) pending.abort.signal.removeEventListener("abort", pending.abort.listener);
      this.pending.delete(id);
      throw error;
    }
    return response;
  }

  async notify(method: string, params: unknown): Promise<void> {
    await this.write({ jsonrpc: "2.0", method, params });
  }

  async close(): Promise<void> {
    if (this.closed) return;
    this.child.stdin.end();
    const exit = once(this.child, "exit");
    const timer = setTimeout(() => this.child.kill("SIGTERM"), 2_000);
    await exit;
    clearTimeout(timer);
  }

  getDiagnostics(): string {
    return this.diagnostics;
  }

  private async write(message: JsonObject): Promise<void> {
    const frame = `${JSON.stringify(message)}\n`;
    if (Buffer.byteLength(frame) > MAX_FRAME_BYTES) {
      throw new ProtocolError("outgoing ACP frame exceeds the 4 MiB limit");
    }
    if (!this.child.stdin.write(frame, "utf8")) await once(this.child.stdin, "drain");
  }

  private acceptLine(line: string): void {
    let value: Record<string, unknown>;
    try {
      value = JSON.parse(line) as Record<string, unknown>;
    } catch {
      this.child.kill("SIGTERM");
      this.failAll(new ProtocolError("AxiomCLI emitted malformed JSON on stdout"));
      return;
    }
    if (value.id !== undefined && ("result" in value || "error" in value)) {
      const numericId = typeof value.id === "number" ? value.id : Number.NaN;
      const pending = this.pending.get(numericId);
      if (!pending) return;
      this.pending.delete(numericId);
      if (pending.timer) clearTimeout(pending.timer);
      if (pending.abort) pending.abort.signal.removeEventListener("abort", pending.abort.listener);
      const error = value.error as { code?: number; message?: string; data?: unknown } | undefined;
      if (error) pending.reject(new AcpError(error.code ?? -32603, error.message ?? "ACP error", error.data));
      else pending.resolve(value.result);
      return;
    }
    if (value.id !== undefined && typeof value.method === "string") {
      const request = value as unknown as RpcRequest;
      let answered = false;
      this.emit("request", {
        request,
        respond: (result: unknown) => {
          if (answered) return;
          answered = true;
          void this.write({ jsonrpc: "2.0", id: request.id, result });
        },
        reject: (code: number, message: string) => {
          if (answered) return;
          answered = true;
          void this.write({ jsonrpc: "2.0", id: request.id, error: { code, message } });
        },
      } satisfies IncomingRequest);
      return;
    }
    if (typeof value.method === "string") this.emit("notification", value as unknown as RpcNotification);
  }

  private acceptChunk(chunk: Buffer): void {
    let start = 0;
    while (start < chunk.length) {
      const newline = chunk.indexOf(0x0a, start);
      const end = newline === -1 ? chunk.length : newline;
      const segment = chunk.subarray(start, end);
      if (this.stdoutRemainder.length + segment.length > MAX_FRAME_BYTES) {
        this.child.kill("SIGTERM");
        this.failAll(new ProtocolError("incoming ACP frame exceeds the 4 MiB limit"));
        return;
      }
      if (segment.length > 0) {
        this.stdoutRemainder = Buffer.concat(
          [this.stdoutRemainder, segment],
          this.stdoutRemainder.length + segment.length,
        );
      }
      if (newline === -1) return;
      const line = this.stdoutRemainder.toString("utf8").replace(/\r$/, "");
      this.stdoutRemainder = Buffer.alloc(0);
      if (line) this.acceptLine(line);
      start = newline + 1;
    }
  }

  private failAll(error: unknown): void {
    for (const pending of this.pending.values()) {
      if (pending.timer) clearTimeout(pending.timer);
      if (pending.abort) pending.abort.signal.removeEventListener("abort", pending.abort.listener);
      pending.reject(error);
    }
    this.pending.clear();
  }
}
