const SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

async function yieldToUi(): Promise<void> {
  const scheduler = (globalThis as typeof globalThis & { scheduler?: { yield(): Promise<void> } }).scheduler;
  if (scheduler?.yield) await scheduler.yield();
  else await new Promise<void>(resolve => setTimeout(resolve, 0));
}

/** Compatibility for browser/older inline tool output. Desktop supplies local
 * Blob URLs instead. Decode bounded chunks outside React render, with no
 * per-byte Array.from callback and no single multi-megabyte blocking loop. */
export async function decodeGeneratedPng(data: string, signal?: AbortSignal): Promise<{
  blob: Blob; width: number; height: number;
} | null> {
  if (!data || data.length > 14_000_000 || data.length % 4) return null;
  const parts: Uint8Array<ArrayBuffer>[] = [];
  let total = 0, width = 0, height = 0;
  try {
    for (let offset = 0; offset < data.length; offset += 262_144) {
      await yieldToUi();
      if (signal?.aborted) return null;
      const binary = atob(data.slice(offset, offset + 262_144));
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      total += bytes.length;
      if (total > 10 * 1024 * 1024) return null;
      if (offset === 0) {
        if (bytes.length < 24 || !SIGNATURE.every((n, i) => bytes[i] === n)) return null;
        const view = new DataView(bytes.buffer);
        width = view.getUint32(16); height = view.getUint32(20);
        if (width < 256 || height < 256 || width > 8192 || height > 8192
          || width % 32 || height % 32 || width * height > 1536 * 1536) return null;
      }
      parts.push(bytes);
    }
    if (signal?.aborted) return null;
    return { blob: new Blob(parts, { type: "image/png" }), width, height };
  } catch { return null; }
}
