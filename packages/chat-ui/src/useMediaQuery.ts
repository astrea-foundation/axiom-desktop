import { useSyncExternalStore } from "react";

/** A touchscreen is the primary pointer: phones and tablets. */
export const COARSE_POINTER_QUERY = "(pointer: coarse)";
/** Phones, including landscape: menus become bottom sheets. */
export const TOUCH_SHEET_QUERY = "(pointer: coarse) and (max-width: 640px), (pointer: coarse) and (max-height: 500px)";
/** Narrow screens show settings as a list, then one category. */
export const NARROW_SCREEN_QUERY = "(max-width: 640px)";

/** Server rendering and tests without matchMedia report no match. */
export function useMediaQuery(query: string): boolean {
  return useSyncExternalStore(
    (notify) => {
      if (typeof window === "undefined" || !window.matchMedia) return () => {};
      const list = window.matchMedia(query);
      list.addEventListener("change", notify);
      return () => list.removeEventListener("change", notify);
    },
    () => typeof window !== "undefined" && !!window.matchMedia && window.matchMedia(query).matches,
    () => false,
  );
}
