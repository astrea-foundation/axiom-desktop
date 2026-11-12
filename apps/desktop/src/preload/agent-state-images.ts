import type { ClientState } from "@axiom/axiom-acp-client";
import type { AgentImagePacket, AgentStatePacket } from "../shared/agent-state";

type ImageUrl = { url: string; width: number; height: number; bytes: number; references: number; lazy: boolean };
export type GeneratedImageUrl = { imageUrl: string; width: number; height: number };

/** Document-local Blob URLs loaded through opaque main-process handles.
 * Scope changes revoke resources before exposing the next account's state. */
export class AgentStateImages {
  constructor(private readonly resources = {
    create: (blob: Blob) => URL.createObjectURL(blob),
    revoke: (url: string) => URL.revokeObjectURL(url),
  }, private readonly cacheBytes = 32 * 1024 * 1024) {}
  private generation = -1;
  private revision = -1;
  private context: string | null = null;
  private urls = new Map<string, ImageUrl>();
  private references = new Set<string>();
  private pending = new Map<string, Promise<ImageUrl>>();
  private activeReads = 0;
  private waitingReads: (() => void)[] = [];
  private latest: ClientState | null = null;
  private visible: ClientState | null = null;

  accept(packet: AgentStatePacket): ClientState {
    if (packet.version !== 1) throw new Error("Unsupported Desktop state transport");
    if (packet.generation < this.generation) {
      if (!this.visible) throw new Error("Stale Desktop state context");
      return this.visible;
    }
    if (packet.generation === this.generation && packet.context !== this.context) {
      throw new Error("Desktop image context changed without a new generation");
    }
    if (packet.generation > this.generation || packet.context !== this.context) {
      this.dispose();
      this.generation = packet.generation;
      this.context = packet.context;
    }
    // A full getState response can arrive after a newer event with references
    // only. Recover its same-context assets, while retaining the newer state.
    for (const image of packet.images) {
      if (!this.context || this.urls.has(image.id)) continue;
      const url = this.resources.create(new Blob([image.bytes], { type: "image/png" }));
      this.urls.set(image.id, { url, width: image.width, height: image.height, bytes: image.bytes.byteLength, references: 0, lazy: false });
    }
    if (packet.revision > this.revision) {
      this.revision = packet.revision;
      this.latest = packet.state;
    }
    if (!this.latest) throw new Error("Desktop state is missing");
    const used = new Set<string>();
    const copy = (value: unknown): unknown => {
      if (!value || typeof value !== "object") return value;
      if (Array.isArray(value)) return value.map(copy);
      const block = value as Record<string, unknown>;
      if (block.type === "image") {
        const id = typeof block.axiomImageAsset === "string" ? block.axiomImageAsset : "";
        if (this.context && id && block.lazy === true) {
          used.add(id);
          return { type: "image", mimeType: "image/png", assetId: id, width: block.width, height: block.height };
        }
        const image = this.context ? this.urls.get(id) : undefined;
        if (!image) return { type: "image", mimeType: block.mimeType };
        used.add(id);
        return { type: "image", mimeType: "image/png", imageUrl: image.url, width: image.width, height: image.height };
      }
      return Object.fromEntries(Object.entries(block).map(([key, item]) => [key, copy(item)]));
    };
    this.visible = copy(this.latest) as ClientState;
    this.references = used;
    for (const [id, image] of this.urls) if (!used.has(id)) {
      this.resources.revoke(image.url);
      this.urls.delete(id);
    }
    return this.visible;
  }

  async load(id: string, read: (context: string, id: string) => Promise<AgentImagePacket>): Promise<GeneratedImageUrl> {
    const context = this.context, generation = this.generation;
    const current = () => context !== null && context === this.context && generation === this.generation && this.references.has(id);
    if (!current()) throw new Error("Image is no longer available");
    let entry = this.urls.get(id);
    if (!entry) {
      let pending = this.pending.get(id);
      if (!pending) {
        pending = this.withReadSlot(async () => {
          if (!current()) throw new Error("Image context changed while waiting");
          const packet = await read(context!, id);
          if (!current() || packet.context !== context || packet.generation !== generation || packet.id !== id) {
            throw new Error("Image context changed while loading");
          }
          const url = this.resources.create(new Blob([packet.bytes], { type: "image/png" }));
          const image: ImageUrl = { url, width: packet.width, height: packet.height, bytes: packet.bytes.byteLength, references: 0, lazy: true };
          this.urls.set(id, image);
          return image;
        }).finally(() => { if (this.pending.get(id) === pending) this.pending.delete(id); });
        this.pending.set(id, pending);
      }
      entry = await pending;
    }
    if (!current() || this.urls.get(id) !== entry) throw new Error("Image is no longer available");
    entry.references++;
    this.urls.delete(id); this.urls.set(id, entry);
    this.trim();
    return { imageUrl: entry.url, width: entry.width, height: entry.height };
  }

  release(id: string): void {
    const entry = this.urls.get(id);
    if (entry) entry.references = Math.max(0, entry.references - 1);
    this.trim();
  }

  private trim(): void {
    let idleBytes = [...this.urls.values()].reduce((bytes, entry) => bytes + (entry.lazy && !entry.references ? entry.bytes : 0), 0);
    for (const [id, entry] of this.urls) {
      if (idleBytes <= this.cacheBytes) break;
      if (!entry.lazy || entry.references) continue;
      idleBytes -= entry.bytes;
      this.resources.revoke(entry.url); this.urls.delete(id);
    }
  }

  private async withReadSlot<T>(work: () => Promise<T>): Promise<T> {
    if (this.activeReads < 2) this.activeReads++;
    else await new Promise<void>(resolve => this.waitingReads.push(resolve));
    try { return await work(); }
    finally {
      const next = this.waitingReads.shift();
      if (next) next(); else this.activeReads--;
    }
  }

  dispose(): void {
    for (const image of this.urls.values()) this.resources.revoke(image.url);
    this.urls.clear();
    this.references.clear();
    this.pending.clear();
    this.latest = null;
    this.visible = null;
    this.revision = -1;
  }
}
