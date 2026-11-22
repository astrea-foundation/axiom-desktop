import { useCallback, useLayoutEffect, useState } from "react";
import { isChatLocale, preferredChatLocale, type ChatLocale } from "@axiom/chat-ui";

const STORAGE_KEY = "axiom.desktop.locale";

function initialLocale(): ChatLocale {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (isChatLocale(saved)) return saved;
  } catch { /* Continue with the system preference. */ }
  return preferredChatLocale(navigator.languages);
}

export function useDesktopLocale() {
  const [locale, setLocale] = useState(initialLocale);
  useLayoutEffect(() => { document.documentElement.lang = locale; }, [locale]);
  const selectLocale = useCallback((next: ChatLocale) => {
    setLocale(next);
    try { localStorage.setItem(STORAGE_KEY, next); } catch { /* The current window still updates. */ }
  }, []);
  return { locale, selectLocale };
}
