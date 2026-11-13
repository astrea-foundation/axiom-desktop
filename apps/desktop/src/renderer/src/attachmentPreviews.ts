import { promptBytes, type PromptAttachment } from "@axiom/axiom-acp-client/attachments";

/** Bounded outbox-to-transcript handoff. Each account/runtime owns its cache. */
export class AttachmentPreviews {
  private entries = new Map<string, PromptAttachment[]>();
  private bytes = 0;

  constructor(private maxBytes = 64 * 1024 * 1024) {}

  get(clientItemId: string | undefined): PromptAttachment[] | undefined {
    return clientItemId ? this.entries.get(clientItemId) : undefined;
  }

  remember(clientItemId: string, files: PromptAttachment[]) {
    if (!files.length || this.entries.has(clientItemId)) return;
    const size = promptBytes("", files);
    if (size > this.maxBytes) return;
    while (this.entries.size && (this.bytes + size > this.maxBytes || this.entries.size >= 32)) {
      const oldest = this.entries.keys().next().value!;
      this.bytes -= promptBytes("", this.entries.get(oldest)!);
      this.entries.delete(oldest);
    }
    this.entries.set(clientItemId, files);
    this.bytes += size;
  }
}
