import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { claimAstreaBanner } from "../src/main/astrea-banner";

test("Astrea introduction is claimed once across launches and independently per profile", async () => {
  const root = await mkdtemp(join(tmpdir(), "axiom-astrea-"));
  try {
    const profile = join(root, "profile");
    const claims = await Promise.all(Array.from({ length: 4 }, () => claimAstreaBanner(profile)));
    assert.equal(claims.filter(Boolean).length, 1);
    assert.equal(await claimAstreaBanner(profile), false);
    assert.equal(await claimAstreaBanner(join(root, "another-profile")), true);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("unavailable profile preferences keep the Astrea introduction hidden", async () => {
  const root = await mkdtemp(join(tmpdir(), "axiom-astrea-"));
  try {
    const file = join(root, "not-a-directory");
    await writeFile(file, "fixture");
    assert.equal(await claimAstreaBanner(file), false);
  } finally { await rm(root, { recursive: true, force: true }); }
});
