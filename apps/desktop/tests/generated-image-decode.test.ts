import assert from "node:assert/strict";
import test from "node:test";
import { decodeGeneratedPng } from "../../../packages/chat-ui/src/generatedImage";

function payload(width = 1536, height = 1536, size = 4 * 1024 * 1024) {
  const bytes = Buffer.alloc(size, 3);
  Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]).copy(bytes);
  bytes.writeUInt32BE(width, 16); bytes.writeUInt32BE(height, 20);
  return bytes;
}

test("large inline compatibility images yield to input tasks and preserve exact bytes", async () => {
  const source = payload();
  let ticks = 0;
  const interval = setInterval(() => ticks++, 0);
  try {
    const decoded = await decodeGeneratedPng(source.toString("base64"));
    assert.ok(decoded);
    assert.ok(ticks > 1, "other event-loop tasks run during decoding");
    assert.equal(decoded.width, 1536); assert.equal(decoded.height, 1536);
    assert.deepEqual(Buffer.from(await decoded.blob.arrayBuffer()), source);
  } finally { clearInterval(interval); }
});

test("cancelled decode and unsupported geometry produce no image", async () => {
  const controller = new AbortController();
  const work = decodeGeneratedPng(payload().toString("base64"), controller.signal);
  controller.abort();
  assert.equal(await work, null);
  assert.equal(await decodeGeneratedPng(payload(2080, 2048, 1024).toString("base64")), null);
  assert.equal(await decodeGeneratedPng("invalid"), null);
});


test("2K PNGs above the former byte limit keep their original bytes", async () => {
  const source = payload(2048, 2048, 11 * 1024 * 1024);
  const decoded = await decodeGeneratedPng(source.toString("base64"));
  assert.ok(decoded);
  assert.deepEqual([decoded.width, decoded.height], [2048, 2048]);
  assert.deepEqual(Buffer.from(await decoded.blob.arrayBuffer()), source);
});
