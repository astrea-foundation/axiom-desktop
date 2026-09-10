import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";
import { describe, expect, it } from "vitest";

const SOURCE_ROOT = new URL(".", import.meta.url).pathname;
const WORKER = new URL("../worker.ts", import.meta.url).pathname;

describe("hosted auth boundary", () => {
  it("contains no browser persistence, legacy chat API, or unsafe rendering seam", () => {
    const files = sourceFiles(SOURCE_ROOT).filter((path) => !path.endsWith(".test.ts"));
    files.push(WORKER);
    const banned = [
      "localStorage",
      "sessionStorage",
      "indexedDB",
      "dangerouslySetInnerHTML",
      ".innerHTML",
      "/api/v1/relay",
      "/api/v1/chats",
      "accountIdentity",
      "fundingContract",
      "console.log",
      "console.error",
    ];

    for (const path of files) {
      const source = readFileSync(path, "utf8");
      for (const token of banned) {
        expect(source, `${relative(SOURCE_ROOT, path)} contains ${token}`).not.toContain(token);
      }
    }
  });
});

function sourceFiles(directory: string): string[] {
  const result: string[] = [];
  for (const entry of readdirSync(directory)) {
    const path = join(directory, entry);
    if (statSync(path).isDirectory()) result.push(...sourceFiles(path));
    else if (/\.tsx?$/.test(path)) result.push(path);
  }
  return result;
}
