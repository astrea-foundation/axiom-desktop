import { EventEmitter, once } from "node:events";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { join } from "node:path";
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
  private readonly terminated: Promise<void>;
  private resolveTerminated!: () => void;
  private readonly processGroupId: number | null;
  private posixGroupForceCleaned = false;

  constructor(options: SidecarProcessOptions) {
    super();
    this.terminated = new Promise((resolve) => {
      this.resolveTerminated = resolve;
    });
    this.child = spawn(options.command, options.args ?? ["acp"], {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
      shell: false,
      detached: process.platform !== "win32",
    });
    this.processGroupId = process.platform !== "win32"
      && this.child.pid !== undefined
      && this.child.pid > 1
      && this.child.pid !== process.pid
      ? this.child.pid
      : null;

    this.child.stdout.on("data", (chunk: Buffer) => this.acceptChunk(chunk));
    this.child.stderr.on("data", (chunk: Buffer) => {
      this.diagnostics = `${this.diagnostics}${chunk.toString("utf8")}`.slice(
        -MAX_DIAGNOSTIC_BYTES,
      );
      this.emit("diagnostic", this.diagnostics);
    });
    this.child.once("error", (error) => this.finish(error));
    this.child.once("exit", (code, signal) => {
      const error = new SidecarExitedError(code, signal, this.diagnostics);
      this.finish(error);
    });
    this.child.once("close", (code, signal) => {
      if (!this.closed) this.finish(new SidecarExitedError(code, signal, this.diagnostics));
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
    // `write()` is awaited before this inner response is returned from the
    // async method. A very fast sidecar exit can reject it in that small gap;
    // attach a handler immediately while preserving rejection for the caller.
    void response.catch(() => undefined);
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
    if (this.closed) {
      this.terminatePosixProcessGroup(true);
      return;
    }
    this.child.stdin.end();
    const gracefulTimer = setTimeout(() => this.terminateChild(false), 2_000);
    const forceTimer = setTimeout(() => this.terminateChild(true), 4_000);
    let deadlineTimer: NodeJS.Timeout;
    const deadline = new Promise<void>((resolve) => {
      deadlineTimer = setTimeout(resolve, 5_000);
    });
    await Promise.race([
      this.terminated,
      deadline,
    ]);
    // The sidecar can exit before a stubborn tool or MCP grandchild. Its
    // detached POSIX process group remains addressable by the original PGID,
    // so close always performs one final group cleanup before returning.
    this.terminatePosixProcessGroup(true);
    clearTimeout(gracefulTimer);
    clearTimeout(forceTimer);
    clearTimeout(deadlineTimer!);
    if (!this.closed) {
      this.finish(new ProtocolError("AxiomCLI did not exit after shutdown"));
    }
  }

  getDiagnostics(): string {
    return this.diagnostics;
  }

  private async write(message: JsonObject): Promise<void> {
    if (this.closed || this.child.stdin.destroyed || !this.child.stdin.writable) {
      throw new ProtocolError("AxiomCLI is not running");
    }
    const frame = `${JSON.stringify(message)}\n`;
    if (Buffer.byteLength(frame) > MAX_FRAME_BYTES) {
      throw new ProtocolError("outgoing ACP frame exceeds the 4 MiB limit");
    }
    if (!this.child.stdin.write(frame, "utf8")) {
      await Promise.race([
        once(this.child.stdin, "drain").then(() => undefined),
        this.terminated.then(() => {
          throw new ProtocolError("AxiomCLI exited while an ACP frame was being written");
        }),
      ]);
    }
  }

  private acceptLine(line: string): void {
    let value: Record<string, unknown>;
    try {
      value = JSON.parse(line) as Record<string, unknown>;
    } catch {
      this.abort(new ProtocolError("AxiomCLI emitted malformed JSON on stdout"));
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
        this.abort(new ProtocolError("incoming ACP frame exceeds the 4 MiB limit"));
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

  private finish(error: Error): void {
    if (this.closed) return;
    this.terminatePosixProcessGroup(true);
    this.closed = true;
    this.failAll(error);
    this.resolveTerminated();
    this.emit("exit", error);
  }

  private abort(error: Error): void {
    this.terminateChild(true);
    this.finish(error);
  }

  private terminateChild(force: boolean): void {
    if (process.platform === "win32" && this.child.pid) {
      const taskkillExecutable = process.env.SystemRoot
        ? join(process.env.SystemRoot, "System32", "taskkill.exe")
        : "taskkill.exe";
      const taskkill = spawn(
        taskkillExecutable,
        ["/PID", String(this.child.pid), "/T", ...(force ? ["/F"] : [])],
        { stdio: "ignore", windowsHide: true, shell: false },
      );
      let fellBack = false;
      const fallback = () => {
        if (fellBack) return;
        fellBack = true;
        this.child.kill(force ? "SIGKILL" : "SIGTERM");
      };
      taskkill.once("error", fallback);
      taskkill.once("exit", (code) => {
        if (code !== 0) fallback();
      });
      taskkill.unref();
      return;
    }
    const signal = force ? "SIGKILL" : "SIGTERM";
    if (this.terminatePosixProcessGroup(force)) return;
    this.child.kill(signal);
  }

  private terminatePosixProcessGroup(force: boolean): boolean {
    if (this.processGroupId === null) return false;
    // A PGID is only an integer and may eventually be reused after its leader
    // exits. Never signal it again after the one definitive SIGKILL cleanup.
    if (force) {
      if (this.posixGroupForceCleaned) return true;
      this.posixGroupForceCleaned = true;
    }
    try {
      process.kill(-this.processGroupId, force ? "SIGKILL" : "SIGTERM");
      return true;
    } catch {
      return false;
    }
  }
}
