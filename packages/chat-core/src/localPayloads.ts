import type { PromptAttachment } from "./viewTypes";

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
  /** Enumerate only one owner's legacy records for a host-controlled migration. */
  async entries(account: string): Promise<Array<[string, PromptPayload]>> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction("payloads", "readonly");
      const values: Array<[string, PromptPayload]> = [];
      const request = transaction.objectStore("payloads").openCursor();
      request.onsuccess = () => {
        const cursor = request.result;
        if (!cursor) return;
        try {
          const scope: unknown = typeof cursor.key === "string" ? JSON.parse(cursor.key) : null;
          if (Array.isArray(scope) && scope.length === 2 && scope[0] === account && typeof scope[1] === "string") {
            values.push([scope[1], cursor.value as PromptPayload]);
          }
        } catch { /* Unrelated legacy keys are never imported or deleted. */ }
        cursor.continue();
      };
      transaction.oncomplete = () => resolve(values);
      transaction.onabort = () => reject(transaction.error ?? new Error("Local storage migration failed."));
      transaction.onerror = () => reject(transaction.error);
    });
  }
  /** Migration deletes the acknowledged version under the same IDB write lock
   * as its comparison, so an older window cannot replace it between requests. */
  async removeIfUnchanged(account: string, id: string, expected: PromptPayload): Promise<boolean> {
    const db = await this.open();
    return new Promise((resolve, reject) => {
      const transaction = db.transaction("payloads", "readwrite");
      const store = transaction.objectStore("payloads");
      const key = JSON.stringify([account, id]);
      const request = store.get(key);
      let unchanged = false;
      request.onsuccess = () => {
        unchanged = request.result === undefined || JSON.stringify(request.result) === JSON.stringify(expected);
        if (unchanged) store.delete(key);
      };
      transaction.oncomplete = () => resolve(unchanged);
      transaction.onabort = () => reject(transaction.error ?? new Error("Local storage migration failed."));
      transaction.onerror = () => reject(transaction.error);
    });
  }
  get(account: string, id: string): Promise<PromptPayload | undefined> { return this.transact(account, id, "readonly", (store, key) => store.get(key)); }
  async put(account: string, id: string, payload: PromptPayload): Promise<void> { await this.transact(account, id, "readwrite", (store, key) => store.put(payload, key)); }
  async remove(account: string, id: string): Promise<void> { await this.transact(account, id, "readwrite", (store, key) => store.delete(key)); }
}
export const localPayloads = new LocalPayloadStore();
