/** @jsxRuntime automatic */
import { createContext, useContext, useMemo, type ReactNode } from "react";
import { CHAT_LANGUAGES, chatTranslator, isChatLocale, type ChatLocale } from "./messages";

const LocaleContext = createContext({ locale: "en" as ChatLocale, t: chatTranslator("en"), onChange: (_locale: ChatLocale) => {} });

export function ChatLocaleProvider({ locale, onChange, children }: {
  locale: ChatLocale;
  onChange(locale: ChatLocale): void;
  children: ReactNode;
}) {
  const value = useMemo(() => ({ locale, t: chatTranslator(locale), onChange }), [locale, onChange]);
  return <LocaleContext.Provider value={value}>{children}</LocaleContext.Provider>;
}

export function useChatLocale() {
  return useContext(LocaleContext);
}

export function ChatLanguageSelect({ className }: { className?: string }) {
  const { locale, t, onChange } = useChatLocale();
  return <select className={className} aria-label={t("language.label")} value={locale} onChange={event => {
    const value = event.currentTarget.value;
    if (isChatLocale(value)) onChange(value);
  }}>
    {Object.entries(CHAT_LANGUAGES).map(([value, label]) => <option key={value} value={value} lang={value}>{label}</option>)}
  </select>;
}
