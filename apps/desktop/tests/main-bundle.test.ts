import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { resolveConfig } from "electron-vite";
import { build } from "vite";

test("the production main bundle includes shared attachment validation without runtime TypeScript imports", async () => {
  const root = fileURLToPath(new URL("..", import.meta.url));
  const { config } = await resolveConfig({ root, envFile: false, logLevel: "silent" }, "build", "production");
  assert(config?.main, "Desktop must define its main-process build");
  const result = await build({
    ...config.main,
    root,
    configFile: false,
    logLevel: "silent",
    build: { ...config.main.build, write: false },
  });
  assert(!("on" in result), "the regression check must finish a production build");
  const outputs = (Array.isArray(result) ? result : [result]).flatMap((bundle) => bundle.output);
  const chunks = outputs.filter((output) => output.type === "chunk");
  assert(chunks.some((chunk) => chunk.isEntry), "Desktop must emit its main entry point");
  for (const chunk of chunks) {
    for (const dependency of [...chunk.imports, ...chunk.dynamicImports]) {
      assert(!dependency.startsWith("@axiom/"), `Packaged main still imports workspace source: ${dependency}`);
      assert(!/\.[cm]?tsx?(?:\?|$)/.test(dependency), `Packaged main still imports TypeScript: ${dependency}`);
    }
  }
  assert(chunks.some((chunk) => Object.keys(chunk.modules).some((id) =>
    /\/chat-core\/src\/attachmentValidation\.ts$/.test(id.replaceAll("\\", "/")),
  )), "shared attachment validation must be compiled into the main bundle");
});
