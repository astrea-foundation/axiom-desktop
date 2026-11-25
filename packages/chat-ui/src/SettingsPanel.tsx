/** @jsxRuntime automatic */
import { useChatLocale } from "./ChatLocale";
import { ArrowDownToLine, ChartPie, MessagesSquare, Plug, KeyRound, Palette, UserRound } from "lucide-react";
import type { ReactNode } from "react";

/** Add categories here, then their controls in SettingsSheet. */
export const SETTINGS_CATEGORIES = [
  { id: "appearance", label: "settings.appearance", description: "settings.appearanceDescription", icon: Palette },
  { id: "account", label: "settings.account", description: "settings.accountDescription", icon: UserRound },
  { id: "chats", label: "settings.chats", description: "settings.chatsDescription", icon: MessagesSquare },
  { id: "usage", label: "settings.usage", description: "settings.usageDescription", icon: ChartPie },
  { id: "mcp", label: "settings.mcp", description: "settings.mcpDescription", icon: Plug },
  { id: "api-keys", label: "settings.apiKeys", description: "settings.apiKeysDescription", icon: KeyRound },
  { id: "updates", label: "settings.updates", description: "settings.updatesDescription", icon: ArrowDownToLine },
] as const;

export type SettingsCategory = typeof SETTINGS_CATEGORIES[number]["id"];

export function SettingsPanel({ category, active, idPrefix, children }: {
  category: SettingsCategory;
  active: SettingsCategory;
  idPrefix: string;
  children: ReactNode;
}) {
  const { t } = useChatLocale();
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
        <h2 className="text-[22px] font-medium tracking-[-0.025em] text-[var(--color-text-primary)]">{t(label)}</h2>
        <p className="mt-1 text-[13px] leading-5 text-[var(--color-text-tertiary)]">{t(description)}</p>
      </header>
      {children}
    </section>
  );
}
