import { readFile, writeFile } from 'node:fs/promises';
const base = new URL('../', import.meta.url);
const read = (name) => readFile(new URL(name, base), 'utf8');
const font = (await readFile(new URL('fonts/PPCirka-Regular.otf', base))).toString('base64');
const sans = (await readFile(new URL(import.meta.resolve('@fontsource-variable/dm-sans/files/dm-sans-latin-wght-normal.woff2')))).toString('base64');
const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="color-scheme" content="light dark"><title>Axiom · {{title}}</title><style>
${await read('tokens.css')}
@font-face{font-family:'PP Cirka';src:url(data:font/otf;base64,${font}) format('opentype');font-display:swap}
@font-face{font-family:'DM Sans';src:url(data:font/woff2;base64,${sans}) format('woff2');font-weight:100 900;font-display:swap}
*{box-sizing:border-box}body{margin:0;background:var(--canvas);color:var(--text);font-family:'DM Sans',system-ui,sans-serif;min-height:100svh;display:flex;flex-direction:column;padding:34px 40px}header svg{width:130px;height:auto}main{margin:auto;padding:50px 0;width:min(440px,100%);text-align:center}.symbol{width:46px;height:44px;margin:0 auto 27px}.symbol svg{width:100%;height:100%}.eyebrow{font-family:monospace;font-size:10px;letter-spacing:.12em;color:var(--muted);margin:0 0 20px}h1{font-family:'PP Cirka',Georgia,serif;font-size:62px;font-weight:400;line-height:1.03;letter-spacing:-.04em;margin:0 0 24px}p{font-size:14px;line-height:1.7;color:var(--muted)}.note{border-top:1px solid var(--line);margin-top:34px;padding-top:22px;font-size:12px}footer{font-size:10px;color:var(--subtle);border-top:1px solid var(--line);padding-top:24px}.brand-light{display:none}@media(prefers-color-scheme:dark){:root{--canvas:#191a1f;--text:#f4f4f6;--muted:#adaeb7;--subtle:#9698a5;--line:rgba(255,255,255,.085)}.brand-dark{display:none}.brand-light{display:block}}@media(max-width:500px){body{padding:28px}h1{font-size:50px}}
</style></head><body><header aria-label="Axiom"><span class="brand-dark">${await read('assets/axiom-ai-dark-logo.svg')}</span><span class="brand-light">${await read('assets/axiom-ai-white-logo.svg')}</span></header><main><div class="symbol" aria-hidden="true">${await read('assets/axiom-symbol-logo-colored.svg')}</div><p class="eyebrow">DESKTOP CONNECTION</p><h1>{{title}}</h1><p>{{message}}</p><p class="note">{{instruction}}</p></main><footer>Axiom · Private by design. Yours by default.</footer></body></html>`;
const output = new URL('templates/desktop-callback.html', base);
if (process.argv.includes('--check')) {
  if (await readFile(output, 'utf8') !== html) throw new Error('Regenerate the shared desktop callback template.');
} else await writeFile(output, html);
