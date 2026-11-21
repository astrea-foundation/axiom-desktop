import { readFileSync } from "node:fs";
import { mkdir, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  defaultInterfaceSize, interfaceSizeShortcut, isInterfaceSize, minimumWindowSize, stepInterfaceSize,
  type InterfaceSize, type InterfaceSizeKey, type InterfaceSizeState,
} from "../shared/interface-size";

const FILE = "interface-size.json";

/** Read before the first window exists, so it opens at the chosen size. */
export function readInterfaceSize(userData: string, platform: string): InterfaceSize {
  try {
    const stored: unknown = JSON.parse(readFileSync(join(userData, FILE), "utf8"));
    const size = (stored as { size?: unknown } | null)?.size;
    if (isInterfaceSize(size)) return size;
  } catch {
    // Missing or unreadable: this profile has not chosen a size.
  }
  return defaultInterfaceSize(platform);
}

async function writeInterfaceSize(userData: string, size: InterfaceSize): Promise<void> {
  await mkdir(userData, { recursive: true });
  const target = join(userData, FILE);
  await writeFile(`${target}.tmp`, JSON.stringify({ schemaVersion: 1, size }), { mode: 0o600 });
  await rename(`${target}.tmp`, target);
}

/** The parts of a BrowserWindow this needs; tests supply a plain object. */
export type SizedWindow = {
  isDestroyed(): boolean;
  isMaximized(): boolean;
  isFullScreen(): boolean;
  getSize(): number[];
  setSize(width: number, height: number): void;
  setMinimumSize(width: number, height: number): void;
  once(event: "closed", listener: () => void): unknown;
  webContents: {
    setZoomFactor(factor: number): void;
    send(channel: string, state: InterfaceSizeState): void;
    on(event: "did-navigate" | "dom-ready", listener: () => void): unknown;
    on(event: "before-input-event", listener: (event: { preventDefault(): void }, input: InterfaceSizeKey) => void): unknown;
  };
};

/**
 * One size for every window of this profile. The stored file is the only
 * source of truth: Chromium also remembers zoom per origin, so each document
 * is set again as it loads.
 */
export class InterfaceSizeController {
  private size: InterfaceSize;
  private readonly windows = new Set<SizedWindow>();
  private saving: Promise<void> = Promise.resolve();

  constructor(private readonly userData: string, private readonly platform: string) {
    this.size = readInterfaceSize(userData, platform);
  }

  get state(): InterfaceSizeState {
    return { size: this.size, defaultSize: defaultInterfaceSize(this.platform) };
  }

  /** For `webPreferences.zoomFactor`, which sizes the very first paint. */
  get zoomFactor(): number { return this.size / 100; }
  get minimumSize(): { width: number; height: number } { return minimumWindowSize(this.size); }

  attach(window: SizedWindow): void {
    this.windows.add(window);
    window.once("closed", () => this.windows.delete(window));
    // Chromium restores its own remembered zoom when a document commits.
    const apply = () => { if (!window.isDestroyed()) window.webContents.setZoomFactor(this.zoomFactor); };
    window.webContents.on("did-navigate", apply);
    window.webContents.on("dom-ready", apply);
    window.webContents.on("before-input-event", (event, input) => {
      const shortcut = interfaceSizeShortcut(input, this.platform);
      if (!shortcut) return;
      event.preventDefault();
      this.set(shortcut === "reset" ? defaultInterfaceSize(this.platform)
        : stepInterfaceSize(this.size, shortcut === "larger" ? 1 : -1));
    });
    this.applyTo(window);
  }

  set(size: unknown): InterfaceSizeState {
    if (!isInterfaceSize(size)) throw new Error("Unsupported interface size");
    if (size === this.size) return this.state;
    this.size = size;
    for (const window of this.windows) this.applyTo(window);
    // Writes are ordered; a failed save keeps the size for this run only.
    this.saving = this.saving.then(() => writeInterfaceSize(this.userData, size)).catch((error: unknown) => {
      console.error("Could not save the interface size", error);
    });
    return this.state;
  }

  /** Resolves once the latest choice is on disk. */
  saved(): Promise<void> { return this.saving; }

  private applyTo(window: SizedWindow): void {
    if (window.isDestroyed()) return;
    const minimum = this.minimumSize;
    window.setMinimumSize(minimum.width, minimum.height);
    if (!window.isMaximized() && !window.isFullScreen()) {
      // A raised minimum does not grow a window that is already smaller.
      const [width = minimum.width, height = minimum.height] = window.getSize();
      if (width < minimum.width || height < minimum.height) {
        window.setSize(Math.max(width, minimum.width), Math.max(height, minimum.height));
      }
    }
    window.webContents.setZoomFactor(this.zoomFactor);
    window.webContents.send("interface-size:changed", this.state);
  }
}
