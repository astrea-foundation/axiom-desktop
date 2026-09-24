import { ArrowDownToLine, ChartPie, KeyRound, Palette, UserRound } from "lucide-react";
import type { ReactNode } from "react";

/** Add categories here, then their controls in SettingsSheet. */
export const SETTINGS_CATEGORIES = [
  { id: "appearance", label: "Appearance", description: "Make Axiom feel at home on your desktop.", icon: Palette },
  { id: "account", label: "Account", description: "Your identity and sign-in methods.", icon: UserRound },
  { id: "usage", label: "Usage", description: "Your spending, by model.", icon: ChartPie },
  { id: "api-keys", label: "API keys", description: "Connect your tools. Track what each key uses.", icon: KeyRound },
  { id: "updates", label: "Updates", description: "The latest Axiom, ready when you are.", icon: ArrowDownToLine },
] as const;

export type SettingsCategory = typeof SETTINGS_CATEGORIES[number]["id"];

export function SettingsPanel({ category, active, idPrefix, children }: {
  category: SettingsCategory;
  active: SettingsCategory;
  idPrefix: string;
  children: ReactNode;
}) {
  const { label, description } = SETTINGS_CATEGORIES.find((entry) => entry.id === category)!;
  return (
    <section
      role="tabpanel"
      id={`${idPrefix}-panel-${category}`}
      aria-labelledby={`${idPrefix}-tab-${category}`}
      hidden={active !== category}
      tabIndex={0}
      className="h-full min-w-0 overflow-y-auto overscroll-contain p-5 outline-offset-[-3px] sm:p-8"
    >
      <header className="mb-7">
        <h2 className="text-[22px] font-medium tracking-[-0.025em] text-[var(--color-text-primary)]">{label}</h2>
        <p className="mt-1 text-[13px] leading-5 text-[var(--color-text-tertiary)]">{description}</p>
      </header>
      {children}
    </section>
  );
}
