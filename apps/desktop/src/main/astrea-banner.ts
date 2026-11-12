import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

/** One introduction per local app profile, shared by windows and accounts. */
export async function claimAstreaBanner(userData: string): Promise<boolean> {
  try {
    await mkdir(userData, { recursive: true });
    await writeFile(join(userData, "astrea-introduction.json"), JSON.stringify({ schemaVersion: 1 }), {
      flag: "wx", mode: 0o600,
    });
    return true;
  } catch {
    // A prior claim or unavailable preferences must never resurface a promo.
    return false;
  }
}
