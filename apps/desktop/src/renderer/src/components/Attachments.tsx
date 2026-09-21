import { useState } from "react";
import { FileText, Paperclip, X } from "lucide-react";
import type { PromptAttachment } from "@axiom/axiom-acp-client";

export interface AttachmentControl {
  items: PromptAttachment[];
  busy: boolean;
  onAddFiles: (files: File[]) => void;
  onRemove: (index: number) => void;
}

export function AttachmentPreview({ attachment }: { attachment: PromptAttachment }) {
  return attachment.kind === "image"
    ? <img src={`data:${attachment.image.mimeType};base64,${attachment.image.data}`} alt={attachment.name} className="my-2 max-h-72 max-w-full rounded-lg object-contain" />
    : <details className="my-2 text-xs"><summary className="cursor-pointer">{attachment.name} · extracted text</summary><pre className="mt-2 max-h-64 overflow-auto whitespace-pre-wrap break-words">{attachment.text}</pre></details>;
}

export function DraftAttachments({ control }: { control: AttachmentControl }) {
  return <div className="mb-2 flex flex-wrap gap-2" aria-label="Attachments">
    {control.items.map((file, index) => <div key={index} className="flex max-w-full items-center gap-2 rounded-lg bg-[var(--wash-chip)] px-2 py-1 text-xs">
      {file.kind === "image" ? <img src={`data:${file.image.mimeType};base64,${file.image.data}`} alt="" className="h-10 w-10 rounded object-cover" /> : <FileText size={15} />}
      <span className="max-w-48 truncate" title={file.name}>{file.name}</span>
      <button type="button" disabled={control.busy} aria-label={`Remove ${file.name}`} onClick={() => control.onRemove(index)} className="rounded p-1 hover:bg-[var(--wash-chip-hover)]"><X size={13} /></button>
    </div>)}
    {control.busy ? <span className="text-xs text-[var(--color-text-tertiary)]" role="status">Reading files locally…</span> : null}
  </div>;
}

export function StoredAttachments({ threadId, userItemId, names }: { threadId: string; userItemId: string; names: string[] }) {
  const [files, setFiles] = useState<PromptAttachment[] | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  if (!names.length) return null;
  return <div className="mb-2 ml-auto max-w-full rounded-xl bg-[var(--wash-chip)] px-3 py-2 text-xs">
    <button type="button" disabled={busy} className="flex items-center gap-2 text-left" aria-expanded={open} onClick={() => {
      if (files) { setOpen(!open); return; }
      const api = window.axiomDesktop?.agent;
      if (!api?.getAttachments) { setError("Restart Axiom to view attachments."); return; }
      setBusy(true); setError("");
      void api.getAttachments(threadId, userItemId).then((result) => { setFiles(result.attachments); setOpen(true); }).catch((error: unknown) => setError(error instanceof Error ? error.message : "Attachments could not be opened.")).finally(() => setBusy(false));
    }}><Paperclip size={13} /><span className="break-all">{names.join(", ")}</span>{busy ? " · Loading…" : ""}</button>
    {open ? files?.map((file, index) => <AttachmentPreview key={index} attachment={file} />) : null}
    {error ? <p role="alert" className="mt-2">{error}</p> : null}
  </div>;
}

export function attachmentNames(raw: unknown): string[] {
  const attachments = (raw as { metadata?: { attachments?: unknown } } | null)?.metadata?.attachments;
  return Array.isArray(attachments) ? attachments.flatMap((item) => item && typeof item.name === "string" ? [item.name] : []) : [];
}
