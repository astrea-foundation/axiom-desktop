/** @jsxRuntime automatic */
/** @jsxRuntime automatic */
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import rehypeKatex from "rehype-katex";
import { rehypeCodeHighlight } from "./codeHighlight";
import { remarkCurrencyMath } from "./currencyMath";
import { CopyButton } from "./CopyButton";

type HastNode = { type: string; value?: string; tagName?: string; properties?: Record<string, unknown>; children?: HastNode[] };

function hastText(node: HastNode | undefined): string {
  if (!node) return "";
  if (node.type === "text") return node.value ?? "";
  return node.children?.map(hastText).join("") ?? "";
}

function codeLanguage(pre: HastNode | undefined): string | null {
  const code = pre?.children?.find((child) => child.type === "element" && child.tagName === "code");
  const classes = code?.properties?.className;
  if (!Array.isArray(classes)) return null;
  for (const value of classes) {
    const match = typeof value === "string" ? /^(?:language|lang)-(.+)$/.exec(value) : null;
    if (match?.[1]) return match[1];
  }
  return null;
}

// Keep component identities stable: chat interaction/streaming can rerender
// Markdown between pointer-down and click, and must not replace the link DOM.
const markdownComponents: Components = {
  // Fenced code gets a header with its language and a copy button. The copied
  // text is the raw source (highlighting spans flattened), read at click time.
  pre: ({ children, node }) => {
    const language = codeLanguage(node as HastNode | undefined);
    return (
      <div className="code-block">
        <div className="code-block__header">
          <span className="code-block__language">{language ?? "code"}</span>
          <CopyButton text={() => hastText(node as HastNode | undefined).replace(/\n$/, "")}
            label="Copy code" showLabel size={12} className="code-block__copy" />
        </div>
        <pre>{children}</pre>
      </div>
    );
  },
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
