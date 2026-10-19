// Rasterise SVGs with the app's own Chromium (no external SVG tooling needed).
// Usage: node ./scripts/run-electron.mjs electron scripts/rasterize-svg.mjs '<json jobs>'
// job: { svg, out, width, height, fill? } — fill recolours every non-"none" fill.
// Captures come back at the display's scale factor; the caller resizes them.
import { app, BrowserWindow } from "electron";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname } from "node:path";

const jobs = JSON.parse(process.argv[process.argv.length - 1]);

app.whenReady().then(async () => {
  const win = new BrowserWindow({
    show: false, width: 64, height: 64, frame: false, transparent: true,
    backgroundColor: "#00000000", webPreferences: { offscreen: true },
  });
  await win.loadURL("data:text/html;charset=utf-8," + encodeURIComponent(
    `<html><body style="margin:0;background:transparent;overflow:hidden"><img id="i" style="display:block;object-fit:contain"></body></html>`,
  ));
  for (const job of jobs) {
    let svg = await readFile(job.svg, "utf8");
    if (job.fill) svg = svg.replace(/fill="(?!none)[^"]*"/g, `fill="${job.fill}"`);
    win.setContentSize(job.width, job.height);
    const data = Buffer.from(svg).toString("base64");
    await win.webContents.executeJavaScript(`new Promise((resolve, reject) => {
      const img = document.getElementById("i");
      img.style.width = "${job.width}px"; img.style.height = "${job.height}px";
      img.onload = () => resolve(true); img.onerror = () => reject(new Error("svg failed"));
      img.src = "data:image/svg+xml;base64,${data}";
    })`);
    await new Promise((resolve) => setTimeout(resolve, 250));
    const image = await win.webContents.capturePage();
    await mkdir(dirname(job.out), { recursive: true });
    await writeFile(job.out, image.toPNG());
    console.log(`wrote ${job.out} ${image.getSize().width}x${image.getSize().height}`);
  }
  win.destroy();
  app.quit();
});
