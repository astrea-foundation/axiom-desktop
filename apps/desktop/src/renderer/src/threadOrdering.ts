import type { Folder, Thread } from "./types";

function messageTime(value: string | null | undefined): number {
  const time = value ? Date.parse(value) : NaN;
  return Number.isFinite(time) ? time : -Infinity;
}

/** Never substitute updatedAt/Date.now(): opening a thread is not a message. */
export function newestMessageAt(...values: (string | null | undefined)[]): string | null {
  let newest: string | null = null;
  for (const value of values) {
    if (messageTime(value) > messageTime(newest)) newest = value!;
  }
  return newest;
}

export function compareThreadMessages(a: Thread, b: Thread): number {
  const aTime = messageTime(a.lastUserMessageAt);
  const bTime = messageTime(b.lastUserMessageAt);
  if (aTime !== bTime) return aTime > bTime ? -1 : 1;
  return a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
}

/** Use all threads, even when search filters/hides the most recent match. */
export function orderFoldersByMessages(folders: Folder[], threads: Thread[]): Folder[] {
  const latest = new Map<string, number>();
  for (const thread of threads) {
    if (thread.folderId) latest.set(thread.folderId, Math.max(
      latest.get(thread.folderId) ?? -Infinity, messageTime(thread.lastUserMessageAt),
    ));
  }
  return [...folders].sort((a, b) => {
    const aTime = latest.get(a.id) ?? -Infinity;
    const bTime = latest.get(b.id) ?? -Infinity;
    // Stable ties retain persisted folder order, including empty folders.
    return aTime === bTime ? 0 : aTime > bTime ? -1 : 1;
  });
}
