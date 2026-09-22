import { EventEmitter } from "node:events";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { join } from "node:path";
import { AcpError, ProtocolError, SidecarExitedError } from "./errors.js";
import type { JsonObject, RpcNotification, RpcRequest } from "./types.js";

const MAX_FRAME_BYTES = 64 * 1024 * 1024;
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
  unsetEnv?: readonly string[];
  /** Inherit only these parent variables. Omit for the legacy inherit-all behavior. */
  inheritEnv?: readonly string[];
}

/**
 * Variables the native Desktop sidecar may inherit from Electron. This is an
 * allowlist, so provider keys, database URLs, wallet secrets, API keys, and
 * unrelated application credentials never enter AxiomCLI or its tool
 * descendants. The retained values are limited to process execution,
 * platform data directories, locale, system-browser launch, TLS roots, and
 * Linux Secret Service access.
 */
export const DESKTOP_SIDECAR_INHERITED_ENV = [
  "PATH",
  "PATHEXT",
  "SYSTEMROOT",
  "WINDIR",
  "COMSPEC",
  "HOME",
  "USER",
  "LOGNAME",
  "USERNAME",
  "USERDOMAIN",
  "USERPROFILE",
  "HOMEDRIVE",
  "HOMEPATH",
  "APPDATA",
  "LOCALAPPDATA",
  "PROGRAMDATA",
  "TMPDIR",
  "TMP",
  "TEMP",
  "XDG_CONFIG_HOME",
  "XDG_DATA_HOME",
  "XDG_CACHE_HOME",
  "XDG_STATE_HOME",
  "XDG_RUNTIME_DIR",
  "DBUS_SESSION_BUS_ADDRESS",
  "GNOME_KEYRING_CONTROL",
  "DISPLAY",
  "WAYLAND_DISPLAY",
  "XAUTHORITY",
  "LANG",
  "LANGUAGE",
  "LC_ALL",
  "LC_CTYPE",
  "TZ",
  "SHELL",
  "TERM",
  "COLORTERM",
  "SSL_CERT_FILE",
  "SSL_CERT_DIR",
] as const;

function deleteCaseInsensitive(environment: NodeJS.ProcessEnv, name: string): void {
  const normalized = name.toLocaleLowerCase("en-US");
  for (const existing of Object.keys(environment)) {
    if (existing.toLocaleLowerCase("en-US") === normalized) delete environment[existing];
  }
}

function setCaseInsensitive(
  environment: NodeJS.ProcessEnv,
  name: string,
  value: string | undefined,
): void {
  deleteCaseInsensitive(environment, name);
  if (value !== undefined) environment[name] = value;
}

export function createChildEnvironment(
  parent: NodeJS.ProcessEnv,
  overrides: NodeJS.ProcessEnv = {},
  unset: readonly string[] = [],
  inherit: readonly string[] | undefined = undefined,
): NodeJS.ProcessEnv {
  const environment: NodeJS.ProcessEnv = {};
  const inheritedNames = inherit
    ? new Set(inherit.map((name) => name.toLocaleLowerCase("en-US")))
    : null;
  for (const [name, value] of Object.entries(parent)) {
    if (inheritedNames && !inheritedNames.has(name.toLocaleLowerCase("en-US"))) continue;
    setCaseInsensitive(environment, name, value);
  }
  for (const [name, value] of Object.entries(overrides)) {
    setCaseInsensitive(environment, name, value);
  }
  for (const name of unset) deleteCaseInsensitive(environment, name);
  return environment;
}

export class AcpProcess extends EventEmitter {
  readonly child: ChildProcessWithoutNullStreams;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();
  private diagnostics = "";
  private closed = false;
  private terminalError: Error | null = null;
  private terminationPublished = false;
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
      env: createChildEnvironment(
        process.env,
        options.env,
        options.unsetEnv,
        options.inheritEnv,
      ),
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
    // A child can close its read end while remaining alive. Without an error
    // listener, the resulting EPIPE is emitted by stdin as an uncaught
    // exception even when the write's callback/promise is otherwise handled.
    this.child.stdin.on("error", (error) => {
      this.finish(this.writeError(error), true);
    });
    this.child.once("error", (error) => this.finish(error, true));
    this.child.once("exit", (code, signal) => {
      const error = new SidecarExitedError(code, signal, this.diagnostics);
      this.finish(error, false);
    });
    this.child.once("close", (code, signal) => {
      if (!this.closed) this.finish(new SidecarExitedError(code, signal, this.diagnostics), false);
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
      await this.terminated;
      return;
    }
    try {
      this.child.stdin.end();
    } catch (error) {
      this.finish(this.writeError(error), true);
    }
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
      this.finish(new ProtocolError("AxiomCLI did not exit after shutdown"), true);
    }
    await this.terminated;
  }

  getDiagnostics(): string {
    return this.diagnostics;
  }

  private async write(message: JsonObject): Promise<void> {
    if (this.closed) {
      throw this.terminalError ?? new ProtocolError("AxiomCLI is not running");
    }
    if (this.child.stdin.destroyed || !this.child.stdin.writable) {
      const error = new ProtocolError("AxiomCLI input stream is not writable");
      throw this.finish(error, true);
    }
    const frame = `${JSON.stringify(message)}\n`;
    if (Buffer.byteLength(frame) > MAX_FRAME_BYTES) {
      throw new ProtocolError("outgoing ACP frame exceeds the 64 MiB limit");
    }
    await new Promise<void>((resolve, reject) => {
      let settled = false;
      const complete = (error?: Error | null) => {
        if (settled) return;
        settled = true;
        if (!error) {
          resolve();
          return;
        }
        reject(this.finish(this.writeError(error), true));
      };
      try {
        this.child.stdin.write(frame, "utf8", complete);
      } catch (error) {
        complete(error instanceof Error ? error : new Error(String(error)));
      }
    });
  }

  private acceptLine(line: string): void {
    let value: Record<string, unknown>;
    try {
      value = JSON.parse(line) as Record<string, unknown>;
    } catch {
      this.finish(new ProtocolError("AxiomCLI emitted malformed JSON on stdout"), true);
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
          void this.write({ jsonrpc: "2.0", id: request.id, result }).catch(() => undefined);
        },
        reject: (code: number, message: string) => {
          if (answered) return;
          answered = true;
          void this.write({ jsonrpc: "2.0", id: request.id, error: { code, message } }).catch(
            () => undefined,
          );
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
        this.finish(new ProtocolError("incoming ACP frame exceeds the 64 MiB limit"), true);
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

  private finish(error: Error, terminate: boolean): Error {
    if (this.closed) return this.terminalError ?? error;
    this.closed = true;
    this.terminalError = error;
    this.failAll(error);
    if (process.platform === "win32") {
      // Do not announce termination (and let Desktop restart) until Windows
      // has finished a best-effort process-tree cleanup. The sidecar leader
      // may have exited while a tool grandchild still owns the SQLite files.
      void this.terminateWindowsTree(true).finally(() => this.publishTermination(error));
    } else {
      if (terminate) this.terminateChild(true);
      else this.terminatePosixProcessGroup(true);
      this.publishTermination(error);
    }
    return error;
  }

  private publishTermination(error: Error): void {
    if (this.terminationPublished) return;
    this.terminationPublished = true;
    this.resolveTerminated();
    this.emit("exit", error);
  }

  private writeError(error: unknown): ProtocolError {
    const detail = error instanceof Error ? error.message : String(error);
    return new ProtocolError(`could not write to AxiomCLI: ${detail}`);
  }

  private terminateChild(force: boolean): void {
    if (process.platform === "win32") {
      void this.terminateWindowsTree(force);
      return;
    }
    const signal = force ? "SIGKILL" : "SIGTERM";
    if (this.terminatePosixProcessGroup(force)) return;
    this.child.kill(signal);
  }

  private terminateWindowsTree(force: boolean): Promise<void> {
    const pid = this.child.pid;
    if (!pid) {
      this.child.kill(force ? "SIGKILL" : "SIGTERM");
      return Promise.resolve();
    }
    const taskkillExecutable = process.env.SystemRoot
      ? join(process.env.SystemRoot, "System32", "taskkill.exe")
      : "taskkill.exe";
    return new Promise<void>((resolve) => {
      let settled = false;
      const complete = (fallback: boolean) => {
        if (settled) return;
        settled = true;
        clearTimeout(timeout);
        if (fallback) this.child.kill(force ? "SIGKILL" : "SIGTERM");
        resolve();
      };
      const taskkill = spawn(
        taskkillExecutable,
        ["/PID", String(pid), "/T", ...(force ? ["/F"] : [])],
        { stdio: "ignore", windowsHide: true, shell: false },
      );
      const timeout = setTimeout(() => {
        taskkill.kill();
        complete(true);
      }, 3_000);
      taskkill.once("error", () => complete(true));
      taskkill.once("exit", (code) => complete(code !== 0));
    });
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
