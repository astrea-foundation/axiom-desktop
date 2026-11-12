/** @jsxRuntime automatic */
import { Check, Copy } from "lucide-react";
import { useEffect, useRef, useState } from "react";

/**
 * Write text to the clipboard. The async Clipboard API can reject when the
 * document is not focused or the context is not secure, so fall back to a
 * temporary selection that works in Electron and older browsers.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.opacity = "0";
    area.style.pointerEvents = "none";
    document.body.appendChild(area);
    area.select();
    try {
      return document.execCommand("copy");
    } catch {
      return false;
    } finally {
      area.remove();
    }
  }
}

export function CopyButton({ text, label = "Copy", className = "", showLabel = false, disabled = false, size = 13 }: {
  /** Read lazily so streamed content is copied as it is at click time. */
  text: string | (() => string);
  label?: string;
  className?: string;
  showLabel?: boolean;
  disabled?: boolean;
  size?: number;
}) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const onClick = async () => {
    const value = typeof text === "function" ? text() : text;
    if (!value || !(await copyText(value))) return;
    setCopied(true);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(false), 1200);
  };
  return (
    <button
      type="button"
      onClick={() => void onClick()}
      disabled={disabled}
      className={className}
      aria-label={copied ? "Copied" : label}
      title={copied ? "Copied" : label}
    >
      {copied ? <Check size={size} aria-hidden="true" /> : <Copy size={size} aria-hidden="true" />}
      {showLabel ? <span>{copied ? "Copied" : "Copy"}</span> : null}
    </button>
  );
}
