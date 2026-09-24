import { useEffect, useState } from "react";

export type ThemePreference = "light" | "dark" | "system";
export type ResolvedTheme = "light" | "dark";

const STORAGE_KEY = "axiom.theme";
const DARK_QUERY = "(prefers-color-scheme: dark)";

function readPreference(): ThemePreference {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored === "light" || stored === "dark" || stored === "system") return stored;
  } catch {
    // Storage can be unavailable; fall through to the default.
  }
  return "system";
}

function systemTheme(): ResolvedTheme {
  return typeof matchMedia === "function" && matchMedia(DARK_QUERY).matches ? "dark" : "light";
}

export function resolveTheme(preference: ThemePreference): ResolvedTheme {
  return preference === "system" ? systemTheme() : preference;
}

let preference: ThemePreference = readPreference();
const listeners = new Set<() => void>();

function apply(): void {
  const resolved = resolveTheme(preference);
  document.documentElement.dataset.theme = resolved;
  window.axiomDesktop?.setTheme(preference, resolved);
  for (const listener of listeners) listener();
}

export function setThemePreference(next: ThemePreference, options: { persist?: boolean } = {}): void {
  preference = next;
  if (options.persist !== false) {
    try {
      localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Persisting is a convenience, not a requirement.
    }
  }
  apply();
}

let bootstrapped = false;
function bootstrap(): void {
  if (bootstrapped) return;
  bootstrapped = true;
  apply();
  if (typeof matchMedia === "function") {
    matchMedia(DARK_QUERY).addEventListener("change", () => {
      if (preference === "system") apply();
    });
  }
}

if (typeof document !== "undefined") bootstrap();

export function useTheme(): {
  preference: ThemePreference;
  resolved: ResolvedTheme;
  setPreference: (next: ThemePreference) => void;
} {
  const [, rerender] = useState(0);

  useEffect(() => {
    bootstrap();
    const listener = () => rerender((tick) => tick + 1);
    listeners.add(listener);
    return () => {
      listeners.delete(listener);
    };
  }, []);

  return { preference, resolved: resolveTheme(preference), setPreference: setThemePreference };
}
