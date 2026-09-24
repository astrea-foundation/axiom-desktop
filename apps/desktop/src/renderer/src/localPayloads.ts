import type { PromptAttachment } from "@axiom/axiom-acp-client";

export interface PromptPayload { text: string; attachments: PromptAttachment[] }
export interface PayloadStore {
  get(account: string, id: string): Promise<PromptPayload | undefined>;
  put(account: string, id: string, payload: PromptPayload): Promise<void>;
  remove(account: string, id: string): Promise<void>;
}

/** Large local inputs stay off localStorage and never leave the device here. */
export class LocalPayloadStore implements PayloadStore {
  private database?: Promise<IDBDatabase>;
  private open(): Promise<IDBDatabase> {
    return this.database ??= new Promise((resolve, reject) => {
      if (typeof indexedDB === "undefined") { reject(new Error("Local file storage is unavailable.")); return; }
      const request = indexedDB.open("axiom.local-payloads", 1);
      request.onupgradeneeded = () => request.result.createObjectStore("payloads");
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error);
      request.onblocked = () => reject(new Error("Close other Axiom windows to update local storage."));
    });
  }
  private async transact<T>(account: string, id: string, mode: IDBTransactionMode, operation: (store: IDBObjectStore, key: string) => IDBRequest<T>): Promise<T> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction("payloads", mode);
      const request = operation(transaction.objectStore("payloads"), JSON.stringify([account, id]));
      // Request success precedes commit; a draft can clear only after commit.
      transaction.oncomplete = () => resolve(request.result);
      transaction.onabort = () => reject(transaction.error ?? new Error("Local storage transaction failed."));
      transaction.onerror = () => reject(transaction.error);
    });
  }
  get(account: string, id: string): Promise<PromptPayload | undefined> { return this.transact(account, id, "readonly", (store, key) => store.get(key)); }
  async put(account: string, id: string, payload: PromptPayload): Promise<void> { await this.transact(account, id, "readwrite", (store, key) => store.put(payload, key)); }
  async remove(account: string, id: string): Promise<void> { await this.transact(account, id, "readwrite", (store, key) => store.delete(key)); }
}
export const localPayloads = new LocalPayloadStore();
