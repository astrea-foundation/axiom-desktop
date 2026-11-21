/**
 * Interface size is the window's zoom, in percent: text, icons and spacing
 * scale together. The main process owns the stored choice.
 */
export const INTERFACE_SIZES = [100, 110, 125, 150] as const;
export type InterfaceSize = typeof INTERFACE_SIZES[number];

export type InterfaceSizeApi = {
  /** The size in effect and this platform's default, as chosen by the main process. */
  get: () => Promise<InterfaceSizeState>;
  set: (size: InterfaceSize) => Promise<InterfaceSizeState>;
  /** Also fires for keyboard changes, which never pass through the renderer. */
  onChange: (callback: (state: InterfaceSizeState) => void) => () => void;
};
export type InterfaceSizeState = { size: InterfaceSize; defaultSize: InterfaceSize };

export function isInterfaceSize(value: unknown): value is InterfaceSize {
  return (INTERFACE_SIZES as readonly unknown[]).includes(value);
}

/** The type scale is tuned for macOS; other platforms start one step larger. */
export function defaultInterfaceSize(platform: string): InterfaceSize {
  return platform === "darwin" ? 100 : 110;
}

export function stepInterfaceSize(current: InterfaceSize, direction: 1 | -1): InterfaceSize {
  const index = INTERFACE_SIZES.indexOf(current) + direction;
  return INTERFACE_SIZES[Math.max(0, Math.min(INTERFACE_SIZES.length - 1, index))]!;
}

/** The fields of Electron's `before-input-event` input that the shortcuts read. */
export type InterfaceSizeKey = {
  type: string; key: string; code?: string;
  control: boolean; meta: boolean; alt: boolean; isAutoRepeat?: boolean;
};

/**
 * Cmd on macOS, Ctrl elsewhere, with `+`/`=`, `-` or `0` (main row or numpad).
 * A held key steps once, so it cannot run to the largest size.
 */
export function interfaceSizeShortcut(input: InterfaceSizeKey, platform: string): "larger" | "smaller" | "reset" | null {
  const mac = platform === "darwin";
  if (input.type !== "keyDown" || input.isAutoRepeat || input.alt) return null;
  if (mac ? !input.meta || input.control : !input.control || input.meta) return null;
  if (input.key === "+" || input.key === "=" || input.code === "NumpadAdd") return "larger";
  if (input.key === "-" || input.code === "NumpadSubtract") return "smaller";
  if (input.key === "0" || input.code === "Numpad0") return "reset";
  return null;
}

/**
 * Zoom divides the window's pixels. At 640 CSS pixels wide Settings turns into
 * its phone layout, so the smallest window grows once a size would take the
 * layout under this floor. Only 150% does: its smallest window is 1080 by 720.
 */
export const MINIMUM_WINDOW = { width: 960, height: 640 } as const;
export const MINIMUM_LAYOUT = { width: 720, height: 480 } as const;

export function minimumWindowSize(size: InterfaceSize): { width: number; height: number } {
  return {
    width: Math.max(MINIMUM_WINDOW.width, Math.ceil(MINIMUM_LAYOUT.width * size / 100)),
    height: Math.max(MINIMUM_WINDOW.height, Math.ceil(MINIMUM_LAYOUT.height * size / 100)),
  };
}
