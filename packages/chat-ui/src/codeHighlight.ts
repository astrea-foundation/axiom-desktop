import rehypeHighlight from "rehype-highlight";
import { visit } from "unist-util-visit";

// Register the bundled grammars once, including across streamed Markdown renders.
const highlight = rehypeHighlight({
  detect: true,
  subset: ["python", "javascript", "typescript", "bash", "json", "sql", "rust", "xml", "css", "yaml"],
  plainText: ["text", "txt", "plaintext", "log", "output"],
});

export function rehypeCodeHighlight(): typeof highlight {
  return (tree, file) => {
    // Bound repeated highlighting work during streaming. Oversized blocks stay
    // readable and selectable in full; only their token colours are omitted.
    let remaining = 100_000;
    visit(tree, "element", (node, _index, parent) => {
      if (node.tagName !== "code" || parent?.type !== "element" || parent.tagName !== "pre") return;
      const classes = Array.isArray(node.properties.className) ? node.properties.className : [];
      const labelled = classes.some(value => typeof value === "string" && /^(language|lang)-/.test(value));
      const length = node.children.reduce((sum, child) => sum + (child.type === "text" ? child.value.length : 0), 0);
      if (length > Math.min(remaining, labelled ? 50_000 : 10_000)) {
        node.properties.className = [...classes, "no-highlight"];
      } else {
        remaining -= length;
      }
    });
    return highlight(tree, file);
  };
}
