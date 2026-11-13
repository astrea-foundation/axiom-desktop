import { useEffect, useMemo, useRef, useState } from "react";
import { FileText, ImageIcon, X } from "lucide-react";
import type { PromptAttachment } from "@axiom/axiom-acp-client";
import { ImagePreviewDialog } from "./ImagePreviewDialog";

export interface AttachmentControl {
  items: PromptAttachment[];
  busy: boolean;
  onAddFiles: (files: File[]) => void;
  onRemove: (index: number) => void;
}

export interface AttachmentSummary {
  name: string;
  kind: PromptAttachment["kind"];
}

function AttachmentCard({ summary, attachment, compact, onRemove, disabled }: {
  summary: AttachmentSummary; attachment?: PromptAttachment; compact?: boolean;
  onRemove?: () => void; disabled?: boolean;
}) {
  const image = attachment?.kind === "image" ? attachment.image : null;
  const imageUrl = useMemo(() => image ? `data:${image.mimeType};base64,${image.data}` : undefined, [image]);
  const [preview, setPreview] = useState<{ name: string; src: string } | null>(null);
  const extension = summary.name.split(".").at(-1)?.toUpperCase();
  return <div data-attachment-card data-attachment-kind={summary.kind}
    className={`relative min-w-0 max-w-full shrink-0 overflow-hidden rounded-[14px] border border-[var(--color-border)] bg-[var(--wash-chip)] ${compact ? "w-32" : "w-44"}`}>
    <div className={`flex items-center justify-center overflow-hidden bg-[var(--surface-track)] ${compact ? "h-20" : "h-28"}`}>
      {imageUrl ? <img src={imageUrl} alt={summary.name} draggable={false}
        className="h-full w-full object-contain" />
        : summary.kind === "image" ? <ImageIcon size={28} aria-hidden="true" className="text-[var(--color-text-tertiary)]" />
          : <FileText size={28} aria-hidden="true" className="text-[var(--color-text-tertiary)]" />}
    </div>
    <div className="px-3 py-2">
      <p className="truncate text-[12px] font-medium text-[var(--color-text-primary)]" title={summary.name}>{summary.name}</p>
      <p className="mt-0.5 text-[10px] text-[var(--color-text-tertiary)]">{extension && extension.length <= 8 ? extension : summary.kind === "image" ? "Image" : "File"}</p>
    </div>
    {imageUrl ? <button type="button" aria-label={`View image ${summary.name}`} aria-haspopup="dialog"
      data-preserve-follow onClick={() => setPreview({ name: summary.name, src: imageUrl })}
      className="absolute inset-0 cursor-zoom-in rounded-[14px] focus-visible:outline-2 focus-visible:-outline-offset-2" /> : null}
    {onRemove ? <button type="button" disabled={disabled} aria-label={`Remove ${summary.name}`} onClick={onRemove}
      className="absolute right-1 top-1 rounded-full bg-[var(--surface-float)] p-1 text-[var(--color-text-secondary)] shadow hover:text-[var(--color-text-primary)] disabled:opacity-50"><X size={13} /></button> : null}
    {preview && preview.src === imageUrl && preview.name === summary.name
      ? <ImagePreviewDialog name={preview.name} src={preview.src} onClose={() => setPreview(null)} /> : null}
  </div>;
}

export function AttachmentCards({ files, compact = false }: { files: PromptAttachment[]; compact?: boolean }) {
  if (!files.length) return null;
  return <div className={`mb-2 flex max-w-full flex-wrap gap-2 ${compact ? "" : "justify-end"}`} aria-label="Attachments">
    {files.map((file, index) => <AttachmentCard key={index} summary={file} attachment={file} compact={compact} />)}
  </div>;
}

export function DraftAttachments({ control }: { control: AttachmentControl }) {
  if (!control.items.length && !control.busy) return null;
  return <div className="mb-2 flex max-h-56 flex-wrap gap-2 overflow-y-auto" aria-label="Attachments">
    {control.items.map((file, index) => <AttachmentCard key={index} summary={file} attachment={file} compact
      onRemove={() => control.onRemove(index)} disabled={control.busy} />)}
    {control.busy ? <span className="text-xs text-[var(--color-text-tertiary)]" role="status">Reading files locally…</span> : null}
  </div>;
}

export function StoredAttachments({ threadId, userItemId, summaries, initialFiles }: {
  threadId: string; userItemId: string; summaries: AttachmentSummary[]; initialFiles?: PromptAttachment[];
}) {
  const container = useRef<HTMLDivElement>(null);
  const [files, setFiles] = useState<PromptAttachment[] | undefined>(initialFiles);
  const [error, setError] = useState("");
  const [attempt, setAttempt] = useState(0);
  const previews = initialFiles ?? files;
  const needsPreview = !previews && summaries.some((file) => file.kind !== "file");
  useEffect(() => {
    if (!needsPreview || !container.current) return;
    let active = true;
    const observer = new IntersectionObserver((entries) => {
      if (!active || !entries.some((entry) => entry.isIntersecting)) return;
      observer.disconnect();
      setError("");
      const api = window.axiomDesktop?.agent;
      if (!api?.getAttachments) { setError("Restart Axiom to view attachment previews."); return; }
      void api.getAttachments(threadId, userItemId).then((result) => {
        if (!result.attachments.length) throw new Error("Missing saved attachments");
        if (active) setFiles(result.attachments);
      }).catch(() => { if (active) setError("Preview unavailable."); });
    }, { rootMargin: "256px" });
    observer.observe(container.current);
    return () => { active = false; observer.disconnect(); };
  }, [threadId, userItemId, needsPreview, attempt]);
  const cards = previews ?? summaries;
  if (!cards.length) return null;
  return <div ref={container} className="mb-2 flex max-w-full flex-col items-end gap-2">
    <div className="flex max-w-full flex-wrap justify-end gap-2" aria-label="Attachments">
      {cards.map((summary, index) => <AttachmentCard key={index} summary={summary} attachment={previews?.[index]} />)}
    </div>
    {previews?.map((file, index) => file.kind === "text" ? <details key={index} className="max-w-full text-xs">
      <summary className="cursor-pointer">Read {file.name}</summary>
      <pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words">{file.text}</pre>
    </details> : null)}
    {error ? <p className="text-xs text-[var(--color-text-tertiary)]">{error} <button type="button" className="underline" onClick={() => setAttempt((value) => value + 1)}>Retry preview</button></p> : null}
  </div>;
}

export function attachmentSummaries(raw: unknown): AttachmentSummary[] {
  const attachments = (raw as { metadata?: { attachments?: unknown } } | null)?.metadata?.attachments;
  return Array.isArray(attachments) ? attachments.flatMap((item) => item && typeof item.name === "string"
    ? [{ name: item.name, kind: item.kind === "image" || item.kind === "text" ? item.kind : /\.(png|jpe?g|webp|gif)$/i.test(item.name) ? "image" : "file" } as AttachmentSummary] : []) : [];
}
