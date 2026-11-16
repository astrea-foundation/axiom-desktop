import type { ClientState } from "@axiom/axiom-acp-client";
import type { StatePatch } from "./state-patch";

/** Private main/preload transport. Image bytes never cross contextBridge in
 * state snapshots; preload replaces these opaque references with local URLs.
 *
 * A packet is either complete (`state`) or incremental (`patch`, relative to
 * `revision - 1` of the same generation). Revisions advance only when the
 * projection changes, so a complete packet may repeat the current revision. */
export interface AgentStatePacket {
  version: 2;
  generation: number;
  revision: number;
  context: string | null;
  /** The whole projection at `revision`. */
  state?: ClientState;
  /** Changes from `revision - 1`. Absent with no `state` means nothing changed. */
  patch?: StatePatch;
  images: { id: string; bytes: ArrayBuffer; width: number; height: number }[];
  /** Image handles this revision references. Omitted when unchanged since the previous packet. */
  live?: string[];
}

/** What preload hands to the page: the whole visible state, or ordered patches to it. */
export type AgentStateUpdate = { state: ClientState } | { patches: StatePatch[] };

export interface AgentImagePacket {
  generation: number;
  context: string;
  id: string;
  bytes: ArrayBuffer;
  width: number;
  height: number;
}
