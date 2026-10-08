/** @jsxRuntime automatic */
import { LaptopMinimal } from 'lucide-react';
import { useEffect, useRef, type ReactNode } from 'react';
import { AccountScreen } from './AccountScreen';
import { SETTINGS_CATEGORIES, type SettingsCategory } from './SettingsPanel';

/** Shared settings navigation; hosts supply supported categories and panel controls. */
export function SettingsLayout({ category, onCategory, idPrefix, onClose, supported, storageLabel = 'Threads stay on this device.', children }: {
  category: SettingsCategory; onCategory(category: SettingsCategory): void; idPrefix: string;
  onClose(): void; supported?: readonly SettingsCategory[]; storageLabel?: string; children: ReactNode;
}) {
  const tabs = useRef<HTMLDivElement>(null);
  const categories = SETTINGS_CATEGORIES.filter(entry => !supported || supported.includes(entry.id));
  useEffect(() => { tabs.current?.querySelector<HTMLButtonElement>('[aria-selected="true"]')?.focus({ preventScroll: true }); }, []);
  return <AccountScreen title="Settings" onClose={onClose} wide>
    <div className="shadow-glass-card grid min-h-0 flex-1 grid-cols-[110px_minmax(0,1fr)] overflow-hidden rounded-[22px] bg-[var(--surface-card)] sm:grid-cols-[200px_minmax(0,1fr)]">
      <aside className="flex min-h-0 flex-col border-r border-[var(--color-border)] bg-[var(--wash-row)] p-2.5 sm:p-4">
        <div ref={tabs} role="tablist" aria-label="Settings categories" aria-orientation="vertical" className="min-h-0 space-y-1 overflow-y-auto" onKeyDown={event => {
          const current = categories.findIndex(entry => entry.id === category);
          const step = event.key === 'ArrowDown' ? 1 : event.key === 'ArrowUp' ? -1 : 0;
          const target = event.key === 'Home' ? 0 : event.key === 'End' ? categories.length - 1 : step ? (current + step + categories.length) % categories.length : -1;
          const next = categories[target]; if (!next) return;
          event.preventDefault(); onCategory(next.id); tabs.current?.querySelector<HTMLButtonElement>(`[data-category="${next.id}"]`)?.focus();
        }}>
          {categories.map(({ id: key, label, icon: Icon }) => <button key={key} type="button" role="tab" id={`${idPrefix}-tab-${key}`} data-category={key} aria-controls={`${idPrefix}-panel-${key}`} aria-selected={category === key} tabIndex={category === key ? 0 : -1} onClick={() => onCategory(key)} className={`flex w-full items-center gap-2.5 rounded-[9px] px-2 py-2.5 text-left text-[13px] transition-colors sm:px-3 ${category === key ? 'bg-[var(--wash-chip)] font-medium text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)] hover:bg-[var(--wash-hover)] hover:text-[var(--color-text-primary)]'}`}><Icon size={16} className="shrink-0" aria-hidden="true" />{label}</button>)}
        </div>
        <p className="mt-auto px-3 pb-1 pt-6 text-[11.5px] leading-[18px] text-[var(--color-text-tertiary)]"><LaptopMinimal size={15} className="mb-2" aria-hidden="true" />{storageLabel}</p>
      </aside>
      <div className="min-h-0 min-w-0">{children}</div>
    </div>
  </AccountScreen>;
}
