import { randomUUID } from "node:crypto";
import type { ClientState } from "@axiom/axiom-acp-client";
import type { AgentStatePacket } from "../shared/agent-state";

type Image = { id: string; width: number; height: number; sent: boolean };
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

/** One instance per webContents. It retains references to strings already owned
 * by native state, not additional decoded buffers. Explicit snapshots resend
 * assets so a reloaded preload can recover independently of earlier events. */
export class AgentStateTransport {
  private identity: string | null = null;
  private context: string | null = null;
  private generation = 0;
  private revision = 0;
  private images = new Map<string, Image>();

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
    const seen = new WeakMap<object, unknown>();
    const copy = (value: unknown): unknown => {
      if (!value || typeof value !== "object") return value;
      if (seen.has(value)) return seen.get(value);
      if (Array.isArray(value)) {
        const result: unknown[] = [];
        seen.set(value, result);
        for (const item of value) result.push(copy(item));
        return result;
      }
      const block = value as Record<string, unknown>;
      if (block.type === "image") {
        // Native/tool-supplied URLs and asset IDs are never trusted as local
        // renderer resources. Only this process can mint an image reference.
        const empty = { type: "image", mimeType: block.mimeType };
        if (!this.context || block.mimeType !== "image/png" || typeof block.data !== "string"
          || block.data.length > 14_000_000) return empty;
        const data = block.data;
        let image = this.images.get(data);
        let bytes: Buffer | undefined;
        if (!image) {
          bytes = Buffer.from(data, "base64");
          if (bytes.length < 24 || bytes.length > 10 * 1024 * 1024
            || bytes.toString("base64") !== data || !PNG_SIGNATURE.every((n, i) => bytes![i] === n)) return empty;
          const width = bytes.readUInt32BE(16), height = bytes.readUInt32BE(20);
          if (width < 256 || height < 256 || width > 8192 || height > 8192
            || width % 32 || height % 32 || width * height > 1536 * 1536) return empty;
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
        Object.defineProperty(result, key, { value: copy(item), enumerable: true, configurable: true, writable: true });
      }
      return result;
    };
    const visible = copy(state) as ClientState;
    for (const data of this.images.keys()) if (!used.has(data)) this.images.delete(data);
    return { version: 1, generation: this.generation, revision: ++this.revision,
      context: this.context, state: visible, images: assets };
  }
}
