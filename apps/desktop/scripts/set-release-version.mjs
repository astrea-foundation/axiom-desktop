import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const version = process.argv[2] ?? "";
const stableSemver = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
if (!stableSemver.test(version)) {
  throw new Error(`release tag must contain a stable MAJOR.MINOR.PATCH version: ${version}`);
}

const packagePath = fileURLToPath(new URL("../package.json", import.meta.url));
const cliManifestPath = fileURLToPath(new URL("../../axiomcli/Cargo.toml", import.meta.url));
const cliManifest = await readFile(cliManifestPath, "utf8");
const cliVersion = /^version\s*=\s*"([^"]+)"\s*$/m.exec(cliManifest)?.[1];
if (cliVersion !== version) {
  throw new Error(`release tag ${version} does not match AxiomCLI product version ${cliVersion ?? "unknown"}`);
}

if (process.argv.includes("--check-only")) process.exit(0);

const manifest = JSON.parse(await readFile(packagePath, "utf8"));
manifest.version = version;
await writeFile(packagePath, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
