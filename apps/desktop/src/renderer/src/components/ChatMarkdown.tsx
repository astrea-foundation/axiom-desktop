import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

export function ChatMarkdown({ text }: { text: string }) {
  return (
    <div className="markdown-prose">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{text}</ReactMarkdown>
    </div>
  );
}
