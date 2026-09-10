import { build } from 'esbuild';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { gzipSync } from 'node:zlib';
import { createHash } from 'node:crypto';
import { buildDownloads } from './scripts/build-downloads.mjs';

// Share a single pinned Three runtime between the existing page scenes and
// the dynamically imported card chapter. No third-party module requests.
await rm('dist', { recursive: true, force: true });
const result = await build({
  entryPoints: ['landing.js', 'proof-loader.js'],
  outdir: 'dist',
  bundle: true,
  splitting: true,
  format: 'esm',
  target: ['es2020'],
  minify: true,
  legalComments: 'linked',
  entryNames: '[name]-[hash]',
  chunkNames: '[name]-[hash]',
  metafile: true,
});

// Hash the entry URLs too: a cached entry must never reference a chunk removed
// by the next build. Update the local preview and production HTML together.
const entries = new Map(Object.entries(result.metafile.outputs)
  .filter(([, info]) => info.entryPoint === 'landing.js' || info.entryPoint === 'proof-loader.js')
  .map(([output, info]) => [info.entryPoint.replace(/\.js$/, ''), output]));
let entryCount = 0;
let html = (await readFile('index.html', 'utf8')).replace(
  /(<script\b[^>]*\bsrc=["'])dist\/(landing|proof-loader)(?:-[a-z0-9]+)?\.js(["'][^>]*>\s*<\/script>)/gi,
  (match, before, name, after) => {
    const output = entries.get(name);
    if (!output) throw new Error(`Missing bundled entry: ${name}`);
    entryCount++;
    return `${before}${output}${after}`;
  },
);
if (entryCount !== 2) throw new Error(`Expected both landing script entries; found ${entryCount}`);
// Keep the scene layout in step with its scripts in an already-open preview.
const sceneStyleHash = createHash('sha256').update(await readFile('proof-scene.css')).digest('hex').slice(0, 12);
html = html.replace(/href="proof-scene\.css(?:\?v=[a-z0-9]+)?"/, `href="proof-scene.css?v=${sceneStyleHash}"`);
await writeFile('index.html', html);

// The local preview uses dist/ directly; the hosting provider receives just these public
// files, without node_modules, source modules or development diagnostics.
await rm('.site', { recursive: true, force: true });
await mkdir('.site');
for (const entry of ['index.html', 'demo.html', 'manifesto.html', '404.html', 'pages.css', 'proof-scene.css', 'assets', 'fonts', 'dist']) {
  await cp(entry, `.site/${entry}`, { recursive: true });
}
await cp('../packages/brand/node_modules/@fontsource-variable/dm-sans/files/dm-sans-latin-wght-normal.woff2', '.site/fonts/dm-sans-latin.woff2');
await buildDownloads();

let total = 0;
for (const output of Object.keys(result.metafile.outputs).filter((file) => file.endsWith('.js'))) {
  const bytes = await readFile(output);
  const gzip = gzipSync(bytes).byteLength;
  total += gzip;
  console.log(`${output}: ${(bytes.byteLength / 1024).toFixed(1)} kB, ${(gzip / 1024).toFixed(1)} kB gzip`);
}
console.log(`Total JavaScript: ${(total / 1024).toFixed(1)} kB gzip`);
