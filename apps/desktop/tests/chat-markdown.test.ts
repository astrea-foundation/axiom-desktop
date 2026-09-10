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
  assert.match(html, /<code class="language-tex">\$x\^2\$/);
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
