import type { ClientState } from "@axiom/axiom-acp-client";

/** Private main/preload transport. Image bytes never cross contextBridge in
 * state snapshots; preload replaces these opaque references with local URLs. */
export interface AgentStatePacket {
  version: 1;
  generation: number;
  revision: number;
  context: string | null;
  state: ClientState;
  images: { id: string; bytes: ArrayBuffer; width: number; height: number }[];
}

export interface AgentImagePacket {
  generation: number;
  context: string;
  id: string;
  bytes: ArrayBuffer;
  width: number;
  height: number;
}
