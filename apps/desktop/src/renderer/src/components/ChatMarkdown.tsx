import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeKatex from "rehype-katex";
import { rehypeCodeHighlight } from "../codeHighlight";
import { remarkCurrencyMath } from "../currencyMath";

// Keep component identities stable: chat interaction/streaming can rerender
// Markdown between pointer-down and click, and must not replace the link DOM.
const markdownComponents: Components = {
  ol: ({ start = 1, children, node }) => {
    const count = node?.children.filter((child) => child.type === "element" && child.tagName === "li").length ?? 1;
    const digits = Math.max(String(start).length, String(start + count - 1).length);
    return <ol start={start} style={{ paddingInlineStart: `max(1.5em, ${digits + 2}ch)` }}>{children}</ol>;
  },
  table: ({ children }) => <div className="markdown-table-scroll"><table>{children}</table></div>,
  // Electron blocks in-app navigation. New-window links go through main's
  // HTTP(S)-only handler and open in the default browser.
  a: ({ href, title, children }) => (
    <a href={href} title={title} target="_blank" rel="noopener noreferrer">{children}</a>
  ),
};

export function ChatMarkdown({ text }: { text: string }) {
  return (
    <div className="markdown-prose">
      <ReactMarkdown
        components={markdownComponents}
        remarkPlugins={[remarkGfm, remarkCurrencyMath]}
        rehypePlugins={[[rehypeKatex, {
          // Model output is untrusted. Never allow TeX to inject HTML, open
          // URLs, fetch images, or expand unbounded macros/dimensions.
          trust: false,
          strict: "ignore",
          maxExpand: 1_000,
          maxSize: 20,
          output: "htmlAndMathml",
        }], rehypeCodeHighlight]}
      >{text}</ReactMarkdown>
    </div>
  );
}
