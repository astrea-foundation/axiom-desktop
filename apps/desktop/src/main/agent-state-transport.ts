import { createHash, randomUUID } from "node:crypto";
import type { ClientState } from "@axiom/axiom-acp-client";
import type { AgentImagePacket, AgentStatePacket } from "../shared/agent-state";
import type { ReadImageResponse } from "@axiom/axiom-acp-client";

type ImageRequest = { threadId: string; callId: string; sha256: string };
type Image = { id: string; width: number; height: number; sent: boolean; request?: ImageRequest };
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

function validDimensions(width: unknown, height: unknown): width is number {
  return typeof width === "number" && typeof height === "number" && width >= 256 && height >= 256
    && width <= 8192 && height <= 8192 && width % 32 === 0 && height % 32 === 0 && width * height <= 2048 * 2048;
}

/** One instance per webContents. Native image descriptors become opaque local
 * handles, and originals are read only on demand. Legacy inline state retains
 * its one-time binary transfer and explicit snapshot recovery behavior. */
export class AgentStateTransport {
  private identity: string | null = null;
  private context: string | null = null;
  private generation = 0;
  private revision = 0;
  private images = new Map<string, Image>();

  async readImage(context: string, id: string, read: (request: ImageRequest) => Promise<ReadImageResponse>): Promise<AgentImagePacket> {
    const current = () => context === this.context && this.context !== null
      ? [...this.images.values()].find(image => image.id === id) : undefined;
    const image = current();
    if (!image?.request) throw new Error("Image is no longer available in this document");
    const result = await read(image.request);
    if (current() !== image) throw new Error("Image context changed while loading");
    if (result.mimeType !== "image/png" || result.sha256 !== image.request.sha256
      || result.width !== image.width || result.height !== image.height || result.data.length > 28_000_000) {
      throw new Error("Saved image identity differs");
    }
    const bytes = Buffer.from(result.data, "base64");
    if (bytes.length < 24 || bytes.length > 20 * 1024 * 1024 || bytes.toString("base64") !== result.data
      || !PNG_SIGNATURE.every((n, i) => bytes[i] === n) || bytes.readUInt32BE(16) !== image.width
      || bytes.readUInt32BE(20) !== image.height || createHash("sha256").update(bytes).digest("hex") !== image.request.sha256) {
      throw new Error("Saved image content differs");
    }
    return { generation: this.generation, context, id, bytes: Uint8Array.from(bytes).buffer, width: image.width, height: image.height };
  }

  encode(state: ClientState, fullImages = false): AgentStatePacket {
    const account = state.account;
    const identity = state.connected && state.runtimeInstanceId && account?.state === "valid" && account.account?.id
      ? `${state.runtimeInstanceId}\0${account.account.id}` : null;
    if (identity !== this.identity) {
      this.identity = identity;
      this.context = identity ? randomUUID() : null;
      this.generation++;
      this.images.clear();
    }
    const assets: AgentStatePacket["images"] = [];
    const used = new Set<string>();
    const included = new Set<string>();
    const copy = (value: unknown, threadId?: string, seen = new WeakMap<object, unknown>()): unknown => {
      if (!value || typeof value !== "object") return value;
      if (seen.has(value)) return seen.get(value);
      if (Array.isArray(value)) {
        const result: unknown[] = [];
        seen.set(value, result);
        for (const item of value) result.push(copy(item, threadId, seen));
        return result;
      }
      const block = value as Record<string, unknown>;
      if (block.type === "image") {
        // Native/tool-supplied URLs and asset IDs are never trusted as local
        // renderer resources. Only this process can mint an image reference.
        const empty = { type: "image", mimeType: block.mimeType };
        const resource = block.imageResource as Record<string, unknown> | undefined;
        if (this.context && threadId && block.mimeType === "image/png" && resource
          && typeof resource.callId === "string" && resource.callId.length > 0 && resource.callId.length <= 512
          && typeof resource.sha256 === "string" && /^[a-f0-9]{64}$/.test(resource.sha256)
          && validDimensions(block.width, block.height)) {
          const key = `resource\0${threadId}\0${resource.callId}\0${resource.sha256}`;
          let image = this.images.get(key);
          if (!image) {
            image = { id: randomUUID(), width: block.width, height: block.height as number, sent: false,
              request: { threadId, callId: resource.callId, sha256: resource.sha256 } };
            this.images.set(key, image);
          }
          used.add(key);
          return { type: "image", mimeType: "image/png", axiomImageAsset: image.id, lazy: true, width: image.width, height: image.height };
        }
        if (!this.context || block.mimeType !== "image/png" || typeof block.data !== "string"
          || block.data.length > 28_000_000) return empty;
        const data = block.data;
        let image = this.images.get(data);
        let bytes: Buffer | undefined;
        if (!image) {
          bytes = Buffer.from(data, "base64");
          if (bytes.length < 24 || bytes.length > 20 * 1024 * 1024
            || bytes.toString("base64") !== data || !PNG_SIGNATURE.every((n, i) => bytes![i] === n)) return empty;
          const width = bytes.readUInt32BE(16), height = bytes.readUInt32BE(20);
          if (width < 256 || height < 256 || width > 8192 || height > 8192
            || width % 32 || height % 32 || width * height > 2048 * 2048) return empty;
          image = { id: randomUUID(), width, height, sent: false };
          this.images.set(data, image);
        }
        used.add(data);
        if ((fullImages || !image.sent) && !included.has(image.id)) {
          bytes ??= Buffer.from(data, "base64");
          assets.push({ id: image.id, bytes: Uint8Array.from(bytes).buffer, width: image.width, height: image.height });
          included.add(image.id);
          image.sent = true;
        }
        return { type: "image", mimeType: "image/png", axiomImageAsset: image.id, width: image.width, height: image.height };
      }
      const result: Record<string, unknown> = {};
      seen.set(value, result);
      for (const [key, item] of Object.entries(block)) {
        const content = value === state && key === "sessions"
          ? Object.fromEntries(Object.entries(state.sessions).map(([id, session]) => [id, copy(session, id)]))
          : copy(item, threadId, seen);
        Object.defineProperty(result, key, { value: content, enumerable: true, configurable: true, writable: true });
      }
      return result;
    };
    const visible = copy(state) as ClientState;
    for (const data of this.images.keys()) if (!used.has(data)) this.images.delete(data);
    return { version: 1, generation: this.generation, revision: ++this.revision,
      context: this.context, state: visible, images: assets };
  }
}
