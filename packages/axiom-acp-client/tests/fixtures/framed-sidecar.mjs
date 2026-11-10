import { createInterface } from "node:readline";

const write = (bytes) => new Promise((resolve, reject) => {
  process.stdout.write(bytes, (error) => error ? reject(error) : resolve());
});

for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.method === "fixture/fragmented") {
    const frame = Buffer.from(JSON.stringify({ jsonrpc: "2.0", id: request.id,
      result: { text: "日本語 🌄", data: "A".repeat(8 * 1024 * 1024) } }));
    // Force a pipe boundary inside a UTF-8 character, then stream the image.
    const split = frame.indexOf(Buffer.from("日")) + 1;
    await write(frame.subarray(0, split));
    await new Promise(resolve => setTimeout(resolve, 10));
    for (let offset = split; offset < frame.length; offset += 32768) {
      await write(frame.subarray(offset, offset + 32768));
    }
    await write('\r\n\n\r\n{"jsonrpc":"2.0","method":"fixture/tail","params":{}}\n');
  } else if (request.method === "fixture/oversized") {
    const chunk = Buffer.alloc(65536, 0x20);
    for (let i = 0; i <= 1024; i++) await write(chunk);
  } else if (request.method === "fixture/malformed") {
    await write('invalid\n{"jsonrpc":"2.0","method":"fixture/tail","params":{}}\n');
  }
}
