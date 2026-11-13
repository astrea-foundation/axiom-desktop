import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import rehypeKatex from "rehype-katex";

// Keep component identities stable: chat interaction/streaming can rerender
// Markdown between pointer-down and click, and must not replace the link DOM.
const markdownComponents: Components = {
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
        remarkPlugins={[remarkGfm, remarkMath]}
        rehypePlugins={[[rehypeKatex, {
          // Model output is untrusted. Never allow TeX to inject HTML, open
          // URLs, fetch images, or expand unbounded macros/dimensions.
          trust: false,
          strict: "ignore",
          maxExpand: 1_000,
          maxSize: 20,
          output: "htmlAndMathml",
        }]]}
      >{text}</ReactMarkdown>
    </div>
  );
}
