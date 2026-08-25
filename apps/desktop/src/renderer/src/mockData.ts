import type { Folder, ProviderModel, Thread } from "./types";

export const MODELS: ProviderModel[] = [
  {
    id: "deepseek-v4-flash",
    label: "DeepSeek V4 Flash",
    shortLabel: "DeepSeek V4 Flash",
    providerLabel: "NEAR",
    model: "deepseek-v4-flash",
    supportedReasoningEfforts: ["low", "medium", "high"],
  },
  {
    id: "kimi-k2.5",
    label: "Kimi K2.5",
    shortLabel: "Kimi K2.5",
    providerLabel: "NEAR",
    model: "kimi-k2.5",
    supportedReasoningEfforts: ["low", "medium", "high"],
  },
];

export const INITIAL_FOLDERS: Folder[] = [
  { id: "folder-tee", name: "TEE research", collapsed: false },
];

export const INITIAL_THREADS: Thread[] = [
  {
    id: "thread-near",
    title: "Verify a NEAR attestation",
    folderId: "folder-tee",
    updatedAt: "just now",
    messages: [
      {
        id: "m1",
        role: "user",
        status: "complete",
        content:
          "Walk me through how Axiom verifies a NEAR TEE attestation before a private completion.",
      },
      {
        id: "m2",
        role: "assistant",
        status: "complete",
        content: `Axiom does not send the prompt until the live endpoint proves it is the attested model.

1. **Quote** the hardware evidence bound to this model and key.
2. **Verify** TDX measurements against the published digest.
3. **Bind TLS** so a rotated endpoint fails closed.
4. **Encrypt** the prompt to the model TEE. There is no unattested fallback.`,
      },
      {
        id: "m3",
        role: "user",
        status: "complete",
        content: "Show me the local proxy path too — I want Codex to talk to Axiom without knowing.",
      },
      {
        id: "m4",
        role: "assistant",
        status: "complete",
        content: `Point any OpenAI-compatible client at the loopback sidecar. Axiom handles attestation and E2EE on the way out.

\`\`\`bash
curl http://127.0.0.1:8787/v1/chat/completions \\
  -H "Authorization: Bearer axiom-local" \\
  -d '{"model":"deepseek-v4-flash","messages":[{"role":"user","content":"ping"}]}'
\`\`\`

From Codex or OpenCode it just looks like another OpenAI endpoint.`,
      },
    ],
  },
  {
    id: "thread-device",
    title: "Device linking flow",
    folderId: "folder-tee",
    updatedAt: "12m",
    messages: [
      {
        id: "d1",
        role: "user",
        status: "complete",
        content: "How do I link this desktop to the phone I already signed in on?",
      },
      {
        id: "d2",
        role: "assistant",
        status: "complete",
        content:
          "Open **Link device** on the phone, scan the desktop sigil, and confirm. The wrapping key never leaves the two devices — Axiom only stores ciphertext and a device record.",
      },
    ],
  },
  {
    id: "thread-draft",
    title: "Off the record drafting",
    folderId: null,
    updatedAt: "1h",
    messages: [
      {
        id: "o1",
        role: "user",
        status: "complete",
        content: "Draft a short note I can send without it living in someone else's logs.",
      },
      {
        id: "o2",
        role: "assistant",
        status: "complete",
        content:
          "Here is a tight version. The chat itself stays encrypted to the attested model; nothing here is a substitute for how you send the note afterward.",
      },
    ],
  },
  {
    id: "thread-pricing",
    title: "Private mode pricing",
    folderId: null,
    updatedAt: "Yesterday",
    messages: [
      {
        id: "p1",
        role: "user",
        status: "complete",
        content: "What does private inference actually cost versus the public endpoint?",
      },
    ],
  },
  {
    id: "thread-prompt",
    title: "System prompt for axiomcli",
    folderId: null,
    updatedAt: "Tue",
    messages: [
      {
        id: "s1",
        role: "user",
        status: "complete",
        content: "Help me write a system prompt that keeps axiomcli from leaking workspace paths.",
      },
    ],
  },
];
