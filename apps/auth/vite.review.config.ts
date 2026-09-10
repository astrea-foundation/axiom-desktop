import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
export default defineConfig({
  plugins: [react(), {
    name: "isolated-design-review",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (req.url?.startsWith("/desktop-callback")) {
          const rejected = req.url.includes("rejected");
          const html = readFileSync(new URL("../../packages/brand/templates/desktop-callback.html", import.meta.url), "utf8")
            .replaceAll("{{title}}", rejected ? "That didn’t connect." : "Back to your space.")
            .replaceAll("{{message}}", rejected ? "This Axiom callback was rejected." : "Axiom received your approval. Return to the application to finish connecting.")
            .replaceAll("{{instruction}}", rejected ? "Return to Axiom and start a new sign-in." : "You can close this browser tab.");
          res.setHeader("Content-Type", "text/html; charset=utf-8");
          res.end(html.replace("</body>", '<a class="review-bar" href="/signin" style="position:fixed;bottom:14px;right:20px;color:inherit;font:11px system-ui">Design preview · Back to pages</a></body>'));
          return;
        }
        if (req.headers.accept?.includes("text/html") && !req.url?.startsWith("/@")) req.url = "/review.html";
        next();
      });
    },
  }],
  resolve: { alias: [{ find: /^(\.\.\/|\.\/)api\/client$/, replacement: fileURLToPath(new URL("./review/client.ts", import.meta.url)) }, { find: /^\.\.\/lib\/ethereum$/, replacement: fileURLToPath(new URL("./review/ethereum.ts", import.meta.url)) }] },
  server: { host: "127.0.0.1", port: 4174, strictPort: true },
  build: { outDir: "dist-review", sourcemap: false, rollupOptions: { input: fileURLToPath(new URL("./review.html", import.meta.url)) } },
});
