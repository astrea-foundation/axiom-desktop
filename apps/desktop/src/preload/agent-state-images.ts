import type { ClientState } from "@axiom/axiom-acp-client";
import type { AgentImagePacket, AgentStatePacket, AgentStateUpdate } from "../shared/agent-state";
import { applyStatePatch, mapStatePatch, type StatePatch } from "../shared/state-patch";

type ImageUrl = { url: string; width: number; height: number; bytes: number; references: number; lazy: boolean };
export type GeneratedImageUrl = { imageUrl: string; width: number; height: number };

/** Document-local state and Blob URLs loaded through opaque main-process handles.
 * Scope changes revoke resources before exposing the next account's state.
 *
 * Main sends the whole projection once and patches afterwards. A patch is
 * applied only to the exact revision it was made from; one that arrives
 * without its base waits for a complete packet (`needsSnapshot`). */
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
  // `latest` is main's projection (opaque handles); `visible` is the page's
  // (local URLs). Both change through the same patches, so untouched sessions
  // and rows keep their identity in the page.
  private latest: ClientState | null = null;
  private visible: ClientState | null = null;
  private missing = new Set<string>();
  private waiting = new Map<number, AgentStatePacket>();
  private lost = false;

  /** Patches are waiting for a base this document does not hold. */
  get needsSnapshot(): boolean {
    return this.lost || this.waiting.size > 0;
  }

  /** The page-visible state after this packet. Throws while no complete state has arrived. */
  accept(packet: AgentStatePacket): ClientState {
    const { state } = this.ingest(packet);
    if (!state) throw new Error("Desktop state is missing");
    return state;
  }

  /**
   * Take one packet. `update` describes how the visible state changed (null
   * when it did not); `state` is null until a complete packet has arrived.
   */
  ingest(packet: AgentStatePacket): { state: ClientState | null; update: AgentStateUpdate | null } {
    if (packet.version !== 2) throw new Error("Unsupported Desktop state transport");
    if (packet.generation < this.generation) return { state: this.visible, update: null };
    if (packet.generation === this.generation && packet.context !== this.context) {
      throw new Error("Desktop image context changed without a new generation");
    }
    if (packet.generation > this.generation || packet.context !== this.context) {
      this.dispose();
      this.generation = packet.generation;
      this.context = packet.context;
    }
    let rebuild = false;
    const patches: StatePatch[] = [];
    // Image bytes are registered with the packet that is being applied, so a
    // waiting patch keeps its payload until its turn.
    const images = (item: AgentStatePacket) => {
      for (const image of item.images) {
        if (!this.context || this.urls.has(image.id)) continue;
        const url = this.resources.create(new Blob([image.bytes], { type: "image/png" }));
        this.urls.set(image.id, { url, width: image.width, height: image.height, bytes: image.bytes.byteLength, references: 0, lazy: false });
        // A row already shown without this image now has its bytes.
        if (this.missing.has(image.id)) rebuild = true;
      }
    };
    const patch = (item: AgentStatePacket) => {
      let next: ClientState;
      try { next = applyStatePatch(this.latest!, item.patch!); }
      catch { this.lost = true; return; }
      images(item);
      this.latest = next;
      this.revision = item.revision;
      if (item.live) this.references = new Set(item.live);
      patches.push(item.patch!);
    };
    if (packet.state) {
      // A complete reply can arrive after newer patches. Recover its
      // same-context assets, while retaining the newer state.
      images(packet);
      if (!this.latest || packet.revision > this.revision || (this.lost && packet.revision === this.revision)) {
        this.latest = packet.state;
        this.revision = packet.revision;
        this.lost = false;
        if (packet.live) this.references = new Set(packet.live);
        rebuild = true;
      }
    } else if (packet.patch) {
      if (this.latest && !this.lost && packet.revision === this.revision + 1) patch(packet);
      else if (packet.revision > this.revision) {
        // Bounded: beyond this only a complete packet can catch the document up.
        if (this.waiting.size >= 512) { this.waiting.clear(); this.lost = true; }
        else this.waiting.set(packet.revision, packet);
      }
    }
    for (let next = this.waiting.get(this.revision + 1); this.latest && !this.lost && next; next = this.waiting.get(this.revision + 1)) {
      this.waiting.delete(next.revision);
      patch(next);
    }
    for (const revision of this.waiting.keys()) if (revision <= this.revision) this.waiting.delete(revision);
    if (!this.latest) return { state: null, update: null };

    let update: AgentStateUpdate | null = null;
    if (rebuild || !this.visible) {
      this.missing.clear();
      this.visible = this.show(this.latest) as ClientState;
      update = { state: this.visible };
    } else if (patches.length) {
      const shown = patches.map(item => mapStatePatch(item, value => this.show(value)));
      let visible = this.visible;
      for (const item of shown) visible = applyStatePatch(visible, item);
      this.visible = visible;
      update = { patches: shown };
    }
    for (const [id, image] of this.urls) if (!this.references.has(id)) {
      this.resources.revoke(image.url);
      this.urls.delete(id);
    }
    return { state: this.visible, update };
  }

  /** Replace main's image handles with what this document may use. */
  private show(value: unknown): unknown {
    if (!value || typeof value !== "object") return value;
    if (Array.isArray(value)) return value.map(item => this.show(item));
    const block = value as Record<string, unknown>;
    if (block.type === "image") {
      const id = typeof block.axiomImageAsset === "string" ? block.axiomImageAsset : "";
      if (this.context && id && block.lazy === true) {
        return { type: "image", mimeType: "image/png", assetId: id, width: block.width, height: block.height };
      }
      const image = this.context ? this.urls.get(id) : undefined;
      if (!image) {
        if (id) this.missing.add(id);
        return { type: "image", mimeType: block.mimeType };
      }
      return { type: "image", mimeType: "image/png", imageUrl: image.url, width: image.width, height: image.height };
    }
    return Object.fromEntries(Object.entries(block).map(([key, item]) => [key, this.show(item)]));
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
    this.missing.clear();
    this.waiting.clear();
    this.lost = false;
    this.revision = -1;
  }
}
