import type { PayloadStore } from "@axiom/chat-core/localPayloads";
import type { PromptAttachment } from "@axiom/chat-core/viewTypes";

interface PendingAttachments { account: string; id: string; attachments: PromptAttachment[] }
/** A failed or stale-account write keeps the latest array until its owner can
 * retry. An older completion cannot acknowledge a newer edit. */
export class AttachmentDraftWrites {
  private pending = new Map<string, PendingAttachments>();
  constructor(private store: PayloadStore) {}
  private key(account: string, id: string) { return JSON.stringify([account, id]); }
  private async commit(value: PendingAttachments): Promise<void> {
    const key = this.key(value.account, value.id);
    await this.store.put(value.account, value.id, { text: "", attachments: value.attachments });
    if (this.pending.get(key) === value) this.pending.delete(key);
  }
  write(account: string, id: string, attachments: PromptAttachment[]): Promise<void> {
    const value = { account, id, attachments };
    this.pending.set(this.key(account, id), value);
    return this.commit(value);
  }
  async retry(account: string, id: string): Promise<void> {
    const value = this.pending.get(this.key(account, id));
    if (value) await this.commit(value);
  }
  async flush(account?: string): Promise<void> {
    await Promise.all([...this.pending.values()].filter(value => account === undefined || value.account === account).map(value => this.commit(value)));
  }
}
