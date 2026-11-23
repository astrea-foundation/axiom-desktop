import { createHash, randomUUID } from "node:crypto";
import type { ClientState, StateChange } from "@axiom/axiom-acp-client";
import type { AgentImagePacket, AgentStatePacket } from "../shared/agent-state";
import { diffState, type StatePatch } from "../shared/state-patch";
import type { ReadImageResponse } from "@axiom/axiom-acp-client";

type ImageRequest = { threadId: string; callId: string; sha256: string };
type Image = { id: string; width: number; height: number; sent: boolean; request?: ImageRequest };
const PNG_SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

function validDimensions(width: unknown, height: unknown): width is number {
  return typeof width === "number" && typeof height === "number" && width >= 256 && height >= 256
    && width <= 8192 && height <= 8192 && width % 32 === 0 && height % 32 === 0 && width * height <= 2048 * 2048;
}

// An image block is replaced as a whole, never patched field by field: preload
// maps complete blocks to document-local resources.
const imageBlock = (value: object) => (value as { type?: unknown }).type === "image";
const define = (target: object, key: string, value: unknown) =>
  Object.defineProperty(target, key, { value, enumerable: true, configurable: true, writable: true });

/** One instance per webContents. Native image descriptors become opaque local
 * handles, and originals are read only on demand. Legacy inline state retains
 * its one-time binary transfer and explicit snapshot recovery behavior.
 *
 * The transport keeps the projection it last sent to its window and sends only
 * the difference. The first packet, every account/runtime identity change and
 * every explicit snapshot carry the whole projection. */
export class AgentStateTransport {
  private identity: string | null = null;
  private context: string | null = null;
  private generation = 0;
  private revision = 0;
  private images = new Map<string, Image>();
  private sent: ClientState | null = null;
  // Image keys referenced by each sent session and by everything outside sessions.
  private sessionImages = new Map<string, Set<string>>();
  private live = "";
  private source: object | null = null;
  private sourceRevision = 0;

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

  /** Complete projection with every inline image, for a new or recovering document. */
  snapshot(state: ClientState): AgentStatePacket {
    return this.project(state, undefined, true);
  }

  /**
   * Packet for a state change: the whole projection the first time and after an
   * identity change, otherwise a patch. `change` comes with client emissions
   * and lets untouched sessions be skipped; without it every session is compared.
   */
  encode(state: ClientState, change?: StateChange): AgentStatePacket {
    return this.project(state, change, false);
  }

  private project(state: ClientState, change: StateChange | undefined, full: boolean): AgentStatePacket {
    const account = state.account;
    const identity = state.connected && state.runtimeInstanceId && account?.state === "valid" && account.account?.id
      ? `${state.runtimeInstanceId}\0${account.account.id}` : null;
    if (identity !== this.identity) {
      this.identity = identity;
      this.context = identity ? randomUUID() : null;
      this.generation++;
      this.images.clear();
      // The next document state is never derived from the previous identity's.
      this.sent = null;
      this.sessionImages.clear();
      this.live = "";
    }
    // A hint is trusted only as the direct successor of the emission this
    // transport encoded last; anything else compares every session.
    const continuous = !!change && change.source === this.source && change.revision === this.sourceRevision + 1;
    if (change) { this.source = change.source; this.sourceRevision = change.revision; }
    const previous = this.sent;
    const touched = !full && continuous && change!.sessions ? new Set(change!.sessions) : null;

    const assets: AgentStatePacket["images"] = [];
    const included = new Set<string>();
    const copy = (value: unknown, used: Set<string>, threadId?: string, seen = new WeakMap<object, unknown>()): unknown => {
      if (!value || typeof value !== "object") return value;
      if (seen.has(value)) return seen.get(value);
      if (Array.isArray(value)) {
        const result: unknown[] = [];
        seen.set(value, result);
        for (const item of value) result.push(copy(item, used, threadId, seen));
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
          return { type: "image", mimeType: "image/png", axiomImageAsset: image.id, lazy: true, width: image.width, height: image.height,
            ...(resource.providerId === "chutes" ? { providerId: "chutes" } : {}) };
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
        if ((full || !image.sent) && !included.has(image.id)) {
          bytes ??= Buffer.from(data, "base64");
          assets.push({ id: image.id, bytes: Uint8Array.from(bytes).buffer, width: image.width, height: image.height });
          included.add(image.id);
          image.sent = true;
        }
        return { type: "image", mimeType: "image/png", axiomImageAsset: image.id, width: image.width, height: image.height };
      }
      const result: Record<string, unknown> = {};
      seen.set(value, result);
      for (const [key, item] of Object.entries(block)) define(result, key, copy(item, used, threadId, seen));
      return result;
    };

    const used = new Set<string>();
    const sessionImages = new Map<string, Set<string>>();
    const next = {} as ClientState;
    for (const [key, item] of Object.entries(state)) {
      if (key !== "sessions" || !item || typeof item !== "object") { define(next, key, copy(item, used)); continue; }
      const sessions: ClientState["sessions"] = {};
      for (const [id, session] of Object.entries(item as ClientState["sessions"])) {
        // A session the client did not touch is already projected; its
        // image references stay live without visiting it again.
        const kept = previous && touched && !touched.has(id) ? previous.sessions[id] : undefined;
        const keptImages = kept ? this.sessionImages.get(id) : undefined;
        const images = keptImages ?? new Set<string>();
        define(sessions, id, kept && keptImages ? kept : copy(session, images, id));
        sessionImages.set(id, images);
        for (const image of images) used.add(image);
      }
      define(next, key, sessions);
    }
    for (const key of this.images.keys()) if (!used.has(key)) this.images.delete(key);
    this.sessionImages = sessionImages;

    const patch: StatePatch | undefined = previous ? diffState(previous, next, imageBlock) : undefined;
    if (!previous || patch) { this.revision++; this.sent = next; }
    const handles = [...used].map(key => this.images.get(key)!.id).sort();
    const live = handles.join("\n");
    const liveChanged = live !== this.live;
    this.live = live;
    const packet: AgentStatePacket = { version: 2, generation: this.generation, revision: this.revision, context: this.context, images: assets };
    if (!previous || full) return { ...packet, state: this.sent!, live: handles };
    return { ...packet, ...(patch ? { patch } : {}), ...(liveChanged ? { live: handles } : {}) };
  }
}
