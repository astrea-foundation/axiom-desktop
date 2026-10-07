const COLORS = ["#f5d7cb", "#d7e0f4", "#d6e7df", "#e8dcf0", "#ebe1cb", "#d7e7eb"];

// Keep this deterministic so the account website and native app show the same avatar.
export function accountAvatar(username: string | null | undefined) {
  const name = (username ?? "").normalize("NFC").trim();
  const words = name.split(/\s+/u);
  const letters = words.length > 1
    ? [...(words[0] ?? "")][0] + ([...(words.at(-1) ?? "")][0] ?? "")
    : [...name].slice(0, 2).join("");
  let hash = 2166136261;
  for (const character of name.toLowerCase()) {
    hash = Math.imul(hash ^ character.codePointAt(0)!, 16777619) >>> 0;
  }
  return {initials: [...letters.toUpperCase()].slice(0, 2).join("") || "?", background: COLORS[hash % COLORS.length]!, color: "#1f1f1f"};
}
