import { createHash, randomUUID } from 'node:crypto';
import { open, rename, rm } from 'node:fs/promises';
import type { ReleaseDownload } from '../shared/updates';

export async function readUpdateJson(response: Response, limit = 128 * 1024): Promise<unknown> {
  if (!response.ok || !response.body) throw new Error('Release feed is unavailable');
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > limit) throw new Error('Release feed is too large');
      chunks.push(value);
    }
    return JSON.parse(Buffer.concat(chunks).toString('utf8')) as unknown;
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
}

// A partial download never replaces an existing file. Only the exact length
// and SHA-256 from the accepted manifest allow the temporary file to be renamed.
export async function saveUpdate(
  file: ReleaseDownload, destination: string, signal: AbortSignal,
  progress: (received: number) => void, fetcher: typeof fetch = fetch,
): Promise<void> {
  const temporary = `${destination}.${randomUUID()}.partial`;
  let handle: Awaited<ReturnType<typeof open>> | undefined;
  let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
  try {
    const response = await fetcher(file.url, { signal, redirect: 'error', credentials: 'omit' });
    if (response.status !== 200 || !response.body) throw new Error('Download unavailable');
    reader = response.body.getReader();
    const length = response.headers.get('content-length');
    if (length !== null && Number(length) !== file.bytes) throw new Error('Download size mismatch');
    handle = await open(temporary, 'wx', 0o600);
    const hash = createHash('sha256');
    let received = 0;
    for (;;) {
      signal.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      received += value.byteLength;
      if (received > file.bytes) throw new Error('Download exceeds expected size');
      hash.update(value);
      // FileHandle.write can write fewer bytes than requested.
      let offset = 0;
      while (offset < value.byteLength) {
        const result = await handle.write(value, offset, value.byteLength - offset);
        if (!result.bytesWritten) throw new Error('Could not save download');
        offset += result.bytesWritten;
      }
      progress(received);
    }
    signal.throwIfAborted();
    if (received !== file.bytes || hash.digest('hex') !== file.sha256) throw new Error('Download checksum mismatch');
    if (file.format === 'AppImage') await handle.chmod(0o700);
    await handle.sync();
    await handle.close(); handle = undefined;
    await rename(temporary, destination);
  } finally {
    await reader?.cancel().catch(() => {});
    reader?.releaseLock();
    await handle?.close().catch(() => {});
    await rm(temporary, { force: true }).catch(() => {});
  }
}
