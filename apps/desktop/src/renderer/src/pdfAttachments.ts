import { getDocument, GlobalWorkerOptions } from "pdfjs-dist/legacy/build/pdf.mjs";
import workerUrl from "pdfjs-dist/legacy/build/pdf.worker.min.mjs?url";
import { MAX_PROMPT_BYTES } from "@axiom/axiom-acp-client/attachments";

GlobalWorkerOptions.workerSrc = workerUrl;

/** Only text extraction, in a bundled worker. No document scripts or uploads. */
export async function extractPdf(data: Uint8Array): Promise<string> {
  const task = getDocument({ data, useWorkerFetch: false, disableFontFace: true, useSystemFonts: false, stopAtErrors: true });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const extraction = async () => {
    const document = await task.promise;
    if (document.numPages > 500) throw new Error("Attach a PDF with at most 500 pages.");
    const pages: string[] = [];
    let total = 0;
    let textPages = 0;
    for (let i = 1; i <= document.numPages; i++) {
      const page = await document.getPage(i);
      const content = await page.getTextContent();
      const text = content.items.map((item) => "str" in item ? item.str + (item.hasEOL ? "\n" : " ") : "").join("").trim();
      {
        if (text) textPages++;
        const section = `[Page ${i}]\n${text || "[No extractable text; visual content is not included.]"}`;
        total += new TextEncoder().encode(section).length + 2;
        if (total > MAX_PROMPT_BYTES) throw new Error("Extracted PDF text exceeds 4 MiB. Attach a smaller section.");
        pages.push(section);
      }
      page.cleanup();
    }
    if (!textPages) throw new Error("This PDF has no extractable text. Attach page images or an OCR text export.");
    return "[PDF text extraction. Images and page layout are not included.]\n\n" + pages.join("\n\n");
  };
  try {
    return await Promise.race([extraction(), new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("PDF extraction took too long. Attach a smaller section.")), 60_000); })]);
  } finally { clearTimeout(timer); await task.destroy(); }
}
