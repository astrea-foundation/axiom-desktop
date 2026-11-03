import type { ClientState } from "@axiom/axiom-acp-client";
import type { AgentStatePacket } from "../shared/agent-state";

/** Document-local Blob URLs: no network, filesystem URL or renderer image RPC.
 * Scope changes revoke resources before exposing the next account's state. */
export class AgentStateImages {
  constructor(private readonly resources = {
    create: (blob: Blob) => URL.createObjectURL(blob),
    revoke: (url: string) => URL.revokeObjectURL(url),
  }) {}
  private generation = -1;
  private revision = -1;
  private context: string | null = null;
  private urls = new Map<string, { url: string; width: number; height: number }>();
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
      this.urls.set(image.id, { url, width: image.width, height: image.height });
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
        const image = this.context ? this.urls.get(id) : undefined;
        if (!image) return { type: "image", mimeType: block.mimeType };
        used.add(id);
        return { type: "image", mimeType: "image/png", imageUrl: image.url, width: image.width, height: image.height };
      }
      return Object.fromEntries(Object.entries(block).map(([key, item]) => [key, copy(item)]));
    };
    this.visible = copy(this.latest) as ClientState;
    for (const [id, image] of this.urls) if (!used.has(id)) {
      this.resources.revoke(image.url);
      this.urls.delete(id);
    }
    return this.visible;
  }

  dispose(): void {
    for (const image of this.urls.values()) this.resources.revoke(image.url);
    this.urls.clear();
    this.latest = null;
    this.visible = null;
    this.revision = -1;
  }
}
