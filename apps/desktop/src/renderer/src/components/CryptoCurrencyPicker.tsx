import { Check, ChevronDown } from "lucide-react";
import { useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { CryptoCurrency } from "@axiom/axiom-acp-client";
import { CryptoIcon, cryptoName, cryptoNetwork } from "./CryptoIcon";

export function CryptoCurrencyPicker({ currencies, value, disabled, onChange }: {
  currencies: CryptoCurrency[]; value: string; disabled: boolean; onChange: (value: string) => void;
}) {
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const search = useRef({ text: "", time: 0 });
  const id = useId();
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const [position, setPosition] = useState({ left: 0, top: 0, width: 0, maxHeight: 360 });
  const selected = currencies.findIndex((coin) => coin.code === value);
  const coin = currencies[selected];
  const showNetwork = (item: CryptoCurrency) => item.code.startsWith("usdc") || item.code.startsWith("usdt")
    || currencies.some((other) => other.code !== item.code && cryptoName(other) === cryptoName(item));

  useEffect(() => { if (disabled) setOpen(false); }, [disabled]);
  useEffect(() => {
    if (!open) return;
    const dismiss = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!root.current?.contains(target) && !list.current?.contains(target)) setOpen(false);
    };
    const resize = () => setOpen(false);
    document.addEventListener("pointerdown", dismiss);
    window.addEventListener("resize", resize);
    return () => { document.removeEventListener("pointerdown", dismiss); window.removeEventListener("resize", resize); };
  }, [open]);
  useLayoutEffect(() => {
    if (!open || !trigger.current) return;
    const bounds = trigger.current.getBoundingClientRect();
    const below = window.innerHeight - bounds.bottom - 24;
    const above = bounds.top - 24;
    const upward = below < 280 && above > below;
    const maxHeight = Math.min(360, upward ? above : below);
    setPosition({ left: bounds.left, width: bounds.width, top: upward ? bounds.top - maxHeight - 8 : bounds.bottom + 8, maxHeight });
  }, [open]);
  useEffect(() => { if (open) list.current?.children[active]?.scrollIntoView({ block: "nearest" }); }, [active, open]);

  const expand = (index = Math.max(0, selected)) => { setActive(index); setOpen(true); };
  const choose = (index: number) => {
    const item = currencies[index];
    if (!item) return;
    onChange(item.code); setOpen(false); trigger.current?.focus();
  };
  return <div ref={root}>
    <label id={`${id}-label`} className="mb-2 block text-[12px] font-medium text-[var(--color-text-secondary)]">Crypto</label>
    <button ref={trigger} type="button" role="combobox" autoFocus disabled={disabled}
      aria-labelledby={`${id}-label`} aria-haspopup="listbox" aria-expanded={open} aria-controls={open ? `${id}-list` : undefined}
      aria-activedescendant={open ? `${id}-option-${active}` : undefined}
      onClick={() => open ? setOpen(false) : expand()}
      onKeyDown={(event) => {
        if (["ArrowDown", "ArrowUp", "Home", "End", "Enter", " ", "Escape"].includes(event.key)) {
          if (event.key === "Escape" && !open) return;
          event.preventDefault(); event.stopPropagation();
          if (event.key === "Escape") setOpen(false);
          else if (event.key === "Enter" || event.key === " ") open ? choose(active) : expand();
          else {
            const index = event.key === "Home" ? 0 : event.key === "End" ? currencies.length - 1
              : open ? (active + (event.key === "ArrowDown" ? 1 : -1) + currencies.length) % currencies.length : Math.max(0, selected);
            expand(index);
          }
        } else if (event.key === "Tab") setOpen(false);
        else if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
          event.preventDefault();
          const time = Date.now();
          search.current = { text: (time - search.current.time < 600 ? search.current.text : "") + event.key.toLowerCase(), time };
          const index = currencies.findIndex((item) => cryptoName(item).toLowerCase().startsWith(search.current.text));
          if (index >= 0) expand(index);
        }
      }}
      className="flex min-h-14 w-full items-center gap-3 rounded-xl border border-[var(--color-border)] bg-[var(--wash-row)] px-3.5 py-3 text-left text-[14px] outline-none transition-colors hover:border-[var(--color-border-strong)] focus-visible:border-[var(--color-border-accent)] focus-visible:ring-2 focus-visible:ring-[var(--color-cherry-glow)] disabled:opacity-40">
      {coin ? <><CryptoIcon code={coin.code} /><span className="min-w-0 flex-1 font-medium">{cryptoName(coin)}</span></> : <span className="flex-1">Choose crypto</span>}
      <ChevronDown size={16} aria-hidden="true" className={`text-[var(--color-text-tertiary)] transition-transform ${open ? "rotate-180" : ""}`} />
    </button>
    {open && root.current ? createPortal(<div ref={list} id={`${id}-list`} role="listbox" aria-labelledby={`${id}-label`}
      style={{ position: "fixed", background: "var(--color-bg-surface)", ...position }}
      className="no-drag glass-panel-solid shadow-glass-pop animate-pop-in z-50 overflow-y-auto overscroll-contain rounded-xl border border-[var(--color-border)] p-1.5">
      {currencies.map((item, index) => <div key={item.code} id={`${id}-option-${index}`} role="option" aria-selected={item.code === value}
        onPointerMove={() => setActive(index)} onMouseDown={(event) => event.preventDefault()} onClick={() => choose(index)}
        className={`flex cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5 text-[13px] ${index === active ? "bg-[var(--wash-chip-hover)]" : ""}`}>
        <CryptoIcon code={item.code} className="h-6 w-6" /><span className="min-w-0 flex-1"><span className="block font-medium">{cryptoName(item)}</span>
          {showNetwork(item) ? <span className="mt-0.5 block text-[11px] text-[var(--color-text-tertiary)]">{cryptoNetwork(item)}</span> : null}</span>
        {item.code === value ? <Check size={15} aria-hidden="true" className="text-[var(--color-text-secondary)]" /> : null}
      </div>)}
    </div>, root.current.closest("dialog") ?? root.current) : null}
  </div>;
}
