import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { accessSync, constants, statSync } from "node:fs";
import { isAbsolute } from "node:path";
import { createChildEnvironment, DESKTOP_SIDECAR_INHERITED_ENV } from "@axiom/axiom-acp-client";

export const FACTORY_RESET_CONFIRMATION = "RESET";
const ERROR_ARGUMENT = "--axiom-factory-reset-error=";

export async function checkFactoryResetSupport(executable: string): Promise<void> {
  try {
    await promisify(execFile)(executable, ["factory-reset", "--help"], {
      timeout: 5000, windowsHide: true, maxBuffer: 64 * 1024,
      env: createChildEnvironment(process.env, {}, ["AXIOM_API_KEY", "AXIOM_PROXY_TOKEN"], DESKTOP_SIDECAR_INHERITED_ENV),
    });
  } catch {
    throw new Error("The native reset helper is unavailable. Update or rebuild Axiom and try again.");
  }
}

export function factoryResetError(argv: string[]): string | null {
  const argument = argv.find((argument) => argument.startsWith(ERROR_ARGUMENT));
  if (!argument) return null;
  return Buffer.from(argument.slice(ERROR_ARGUMENT.length, ERROR_ARGUMENT.length + 12_000), "base64url").toString("utf8");
}

export function factoryResetRelaunch(options: {
  sidecar: string;
  executable: string;
  argv: string[];
  userData: string;
  profile: "system" | "staging" | "local";
  profileName?: string;
  development?: boolean;
  developmentRestart?: string;
}): { execPath: string; args: string[] } {
  let executable = options.executable;
  let restartArgs = options.argv.slice(1).filter((arg) => !arg.startsWith(ERROR_ARGUMENT));
  if (options.development) {
    // electron-vite exits with Electron and takes the UI server with it.
    // Reopening Electron alone would leave it pointing at that dead server.
    try {
      const launcher = JSON.parse(options.developmentRestart ?? "null");
      if (!launcher || typeof launcher.executable !== "string" || !isAbsolute(launcher.executable)
        || !Array.isArray(launcher.args) || launcher.args.length < 3 || launcher.args.length > 128
        || !launcher.args.every((arg: unknown) => typeof arg === "string" && arg.length <= 8192)
        || !isAbsolute(launcher.args[0]) || !["vite", "staging"].includes(launcher.args[1]) || launcher.args[2] !== "dev"
        || !statSync(launcher.executable).isFile() || !statSync(launcher.args[0]).isFile()) throw new Error("Invalid launcher");
      accessSync(launcher.executable, process.platform === "win32" ? constants.F_OK : constants.X_OK);
      accessSync(launcher.args[0], constants.R_OK);
      executable = launcher.executable;
      restartArgs = launcher.args;
    } catch {
      throw new Error("The development restart command is unavailable. Restart Axiom with pnpm desktop before resetting it.");
    }
  }
  // Electron starts this native helper only after the current process exits.
  // No Chromium session, SQLite database or credential writer remains open.
  return {
    execPath: options.sidecar,
    args: ["factory-reset", "--confirm", "--profile", options.profile,
      ...(options.profileName ? ["--profile-name", options.profileName] : []),
      "--desktop-user-data", options.userData, "--restart-executable", executable,
      ...restartArgs.map((arg) => `--restart-arg=${arg}`)],
  };
}

export class FactoryResetController {
  private pending = false;

  constructor(private readonly actions: {
    isInstalling(): boolean;
    prepare(): Promise<() => void>;
    stop(): Promise<void>;
    quit(): void;
  }) {}

  isPending(): boolean { return this.pending; }

  async reset(confirmation: unknown): Promise<void> {
    if (confirmation !== FACTORY_RESET_CONFIRMATION) throw new Error("Type RESET to confirm permanent deletion.");
    if (this.pending) throw new Error("Factory reset is already in progress.");
    if (this.actions.isInstalling()) throw new Error("Wait for the update to finish before resetting Axiom.");
    this.pending = true;
    try {
      const relaunch = await this.actions.prepare();
      await this.actions.stop();
      relaunch();
      this.actions.quit();
    } catch (error) {
      this.pending = false;
      throw error;
    }
  }
}
