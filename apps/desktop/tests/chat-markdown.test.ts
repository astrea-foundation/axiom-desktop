import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ChatMarkdown } from "../src/renderer/src/components/ChatMarkdown";

const render = (text: string) => renderToStaticMarkup(createElement(ChatMarkdown, { text }));

test("Markdown links open through Electron's external browser handler", () => {
  const html = render('[Cats](https://example.invalid/cats "Cat resources")\n\n[More][resource]\n\n<https://example.invalid/news>\n\n[resource]: http://example.invalid/more');
  assert.equal(html.match(/target="_blank"/g)?.length, 3);
  assert.equal(html.match(/rel="noopener noreferrer"/g)?.length, 3);
  assert.match(html, /href="https:\/\/example.invalid\/cats" title="Cat resources"/);
  assert.match(html, /href="http:\/\/example.invalid\/more"/);
});

test("external link rendering retains Markdown's unsafe-URL filtering", () => {
  const html = render('[script](javascript:alert%281%29) [file](file:///private.txt) [data](data:text/html,payload)');
  assert.doesNotMatch(html, /href="(?:javascript:|file:|data:)/);
});

test("inline LaTeX renders visually and includes accessible MathML", () => {
  const html = render(String.raw`Energy: $E = mc^2$. Roots: $x = \frac{-b \pm \sqrt{b^2-4ac}}{2a}$.`);
  assert.equal(html.match(/class="katex"/g)?.length, 2);
  assert.match(html, /class="katex-html" aria-hidden="true"/);
  assert.match(html, /<math /);
  assert.match(html, /<mfrac>/);
  assert.doesNotMatch(html, /katex-error/);
});

test("block equations, matrices and aligned expressions render as display math", () => {
  const html = render(String.raw`$$
\int_0^\infty e^{-x^2}\,dx = \frac{\sqrt{\pi}}{2}
$$

$$
\begin{bmatrix}1 & 2 & 3 \\ 4 & 5 & 6 \\ 7 & 8 & 9\end{bmatrix}
$$

$$
\begin{aligned}a &= b + c \\ d &= e + f\end{aligned}
$$`);
  assert.equal(html.match(/class="katex-display"/g)?.length, 3);
  assert.match(html, /<mtable/);
  assert.doesNotMatch(html, /katex-error/);
});

test("code examples and escaped currency stay literal; math fences render", () => {
  const html = render('`$x^2$`\n\n```tex\n$x^2$\n```\n\nCost: \\$5 and \\$10.\n\n```math\nx^2\n```');
  assert.match(html, /<code>\$x\^2\$<\/code>/);
  assert.match(html, /<code class="[^"]*language-tex[^"]*">\$x\^2\$/);
  assert.match(html, /Cost: \$5 and \$10/);
  assert.equal(html.match(/class="katex-display"/g)?.length, 1);
});

test("incomplete streamed math and invalid LaTeX remain readable without throwing", () => {
  assert.match(render(String.raw`Working: $\frac{1}{`), /Working:/);
  const invalid = render(String.raw`$\frac{1}{$`);
  assert.match(invalid, /katex-error/);
  assert.match(invalid, /\\frac/);
  assert.doesNotMatch(render(String.raw`Working: $\frac{1}{2}$`), /katex-error/);
});

test("TeX cannot inject HTML, script links, or remote images", () => {
  const html = render(String.raw`$\href{javascript:alert(1)}{click}$

$\includegraphics{https://example.invalid/tracking.png}$

$\htmlClass{malicious-class}{x}$

<script>alert(1)</script>`);
  assert.doesNotMatch(html, /<script|<img|href="javascript:|class="malicious-class"/);
});

test("recursive macros are bounded and cannot leak definitions to later messages", () => {
  assert.match(render(String.raw`$\def\loop{\loop}\loop$`), /katex-error/);
  render(String.raw`$\gdef\messageOnly{private} \messageOnly$`);
  const later = render(String.raw`$\messageOnly$`);
  assert.match(later, /\\messageOnly/);
  assert.doesNotMatch(later, /private/);
});

test("Python fences and aliases colour keywords, strings, numbers and comments", () => {
  for (const language of ["python", "py"]) {
    const html = render(`\`\`\`${language}\n# precision\ndef pi(digits: int = 100):\n    return "3.14"\n\`\`\``);
    for (const token of ["comment", "keyword", "number", "string"]) {
      assert.match(html, new RegExp(`class="hljs-${token}"`));
    }
    assert.match(html, /hljs-title function_/);
  }
});

test("unlabelled Python is detected, while explicit text and unknown languages stay literal", () => {
  const source = '#!/usr/bin/env python3\ndef pi(digits: int = 100):\n    return "3.14"\n';
  assert.match(render(`\`\`\`\n${source}\`\`\``), /language-python/);
  for (const language of ["text", "txt", "plaintext", "log", "output", "unknown-language"]) {
    const html = render(`\`\`\`${language}\n${source}\`\`\``);
    assert.doesNotMatch(html, /<span class="hljs-/);
    assert.match(html, /def pi\(digits: int = 100\):/);
  }
  assert.equal(render('`return "literal"`'), '<div class="markdown-prose"><p><code>return &quot;literal&quot;</code></p></div>');
});

test("common fenced languages highlight without requiring remote grammars", () => {
  for (const [language, source] of [
    ["js", 'const value = "hello";'],
    ["typescript", 'const value: string = "hello";'],
    ["json", '{"value": 100}'],
    ["bash", 'echo "hello"'],
    ["rust", 'fn main() { let value = 100; }'],
    ["sql", 'SELECT * FROM messages WHERE id = 100;'],
  ]) {
    assert.match(render(`\`\`\`${language}\n${source}\n\`\`\``), /<span class="hljs-/);
  }
});

test("highlighted HTML stays escaped and incomplete streamed fences remain readable", () => {
  const html = render('```html\n<script>alert("hello")</script>\n<img src=x onerror=alert(1)>\n```');
  assert.doesNotMatch(html, /<script|<img|<iframe/);
  assert.match(html, /&lt;/);
  assert.match(html, /hljs-tag/);
  const partial = render('```python\ndef pi():\n    return "unfinished');
  assert.match(partial, /hljs-keyword/);
  assert.match(partial, /unfinished/);
});

test("oversized code stays complete without spending time on token highlighting", () => {
  for (const language of ["python", ""]) {
    const source = '# long example\n'.repeat(4000);
    const html = render(`\`\`\`${language}\n${source}\`\`\``);
    assert.doesNotMatch(html, /<span class="hljs-/);
    assert.equal(html.match(/# long example\n/g)?.length, 4000);
  }
});
