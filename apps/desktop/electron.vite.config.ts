import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, externalizeDepsPlugin } from "electron-vite";

export default defineConfig({
  main: {
    // Both workspace packages export TypeScript; compile the SDK's transitive
    // chat-core imports too, since packaged Node cannot load TS in node_modules.
    plugins: [externalizeDepsPlugin({ exclude: ["@axiom/axiom-acp-client", "@axiom/chat-core"] })],
  },
  preload: {
    plugins: [externalizeDepsPlugin()],
    build: {
      rollupOptions: {
        output: {
          entryFileNames: "index.cjs",
          format: "cjs",
        },
      },
    },
  },
  renderer: {
    resolve: {
      alias: {
        "@renderer": resolve("src/renderer/src"),
      },
    },
    plugins: [react(), tailwindcss()],
  },
});
