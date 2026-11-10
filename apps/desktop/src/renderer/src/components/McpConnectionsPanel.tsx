import { Check, LoaderCircle, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { DesktopMcpAction, DesktopMcpResponse, DesktopMcpServer, DesktopMcpServerInput } from "@axiom/axiom-acp-client";
import { parseLocalMcpConfig } from "../mcpConfig";

const button = "rounded-lg bg-[var(--wash-chip)] px-3 py-2 text-[12px] transition-colors hover:bg-[var(--wash-chip-hover)] disabled:opacity-40";
const input = "w-full rounded-lg border border-[var(--color-border)] bg-[var(--wash-row)] px-3 py-2 text-[13px] outline-none focus:border-[var(--color-border-accent)] disabled:opacity-50";
type Draft = { name: string; command: string; args: string; enabled: boolean; env: Array<{ key: string; value: string }>; replaceEnv: boolean; editing: boolean };
const emptyDraft = (): Draft => ({ name: "", command: "", args: "[]", enabled: true, env: [], replaceEnv: false, editing: false });

export function McpConnectionsPanel() {
  const [state, setState] = useState<DesktopMcpResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [importText, setImportText] = useState<string | null>(null);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  useEffect(() => {
    mounted.current = true;
    void refresh();
    return () => { mounted.current = false; };
  }, []);
  async function request(action: DesktopMcpAction): Promise<DesktopMcpResponse> {
    const api = window.axiomDesktop?.agent;
    if (!api?.desktopMcp) throw new Error("Restart Axiom to load MCP support.");
    const response = await api.desktopMcp({ expectedRevision: state?.revision, action });
    if (mounted.current) setState(response);
    return response;
  }
  async function perform(action: DesktopMcpAction, done?: () => void) {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null);
    try { await request(action); if (mounted.current) done?.(); }
    catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { inFlight.current = false; if (mounted.current) setBusy(false); }
  }
  async function refresh() { await perform({ kind: "list" }); }
  function edit(server: DesktopMcpServer) {
    setImportText(null); setError(null);
    setDraft({ name: server.name, command: server.command, args: JSON.stringify(server.args, null, 2), enabled: server.enabled, env: [], replaceEnv: false, editing: true });
  }
  function serverInput(server: DesktopMcpServer): DesktopMcpServerInput {
    return { name: server.name, command: server.command, args: server.args, enabled: server.enabled };
  }
  function save() {
    if (!draft) return;
    try {
      const args: unknown = JSON.parse(draft.args);
      if (!Array.isArray(args) || args.some((arg) => typeof arg !== "string")) throw new Error("Arguments must be a JSON array of strings.");
      const env: Record<string, string> = Object.create(null) as Record<string, string>;
      for (const row of draft.env) {
        if (!row.key || Object.hasOwn(env, row.key)) throw new Error("Environment variable names must be nonempty and unique.");
        env[row.key] = row.value;
      }
      void perform({ kind: "save", server: { name: draft.name, command: draft.command, args, enabled: draft.enabled, ...(draft.replaceEnv ? { env } : {}) } }, () => setDraft(null));
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }
  return <div className="space-y-4 text-[13px]">
    <p className="text-[var(--color-text-secondary)]">Connect local MCP servers, then choose their tools in each thread’s Agent settings.</p>
    <p className="rounded-xl bg-[var(--wash-chip)] p-3 text-[12px] leading-5 text-[var(--color-text-secondary)]">Local servers run with your computer’s permissions. Only add programs you trust. Connected services can see tool inputs and results; the TEE badge covers model messages. Credentials are stored in your system credential store.</p>
    <div className="flex flex-wrap gap-2">
      <button type="button" className={button} disabled={busy || !state} onClick={() => { setImportText(null); setDraft(emptyDraft()); setError(null); }}><Plus size={13} className="mr-1 inline" />Add local server</button>
      <button type="button" className={button} disabled={busy || !state} onClick={() => { setDraft(null); setImportText(""); setError(null); }}>Import configuration</button>
      <button type="button" className={button} disabled={busy} onClick={() => void refresh()} aria-label="Refresh MCP connections">{busy ? <LoaderCircle size={14} className="animate-spin" /> : <RefreshCw size={14} />}</button>
    </div>
    {error ? <p role="alert" className="text-[var(--color-danger-strong)]">{error}</p> : null}
    {draft ? <form className="space-y-3 rounded-xl border border-[var(--color-border)] p-4" onSubmit={(event) => { event.preventDefault(); save(); }}>
      <h3 className="font-medium">{draft.editing ? "Edit local server" : "Add local server"}</h3>
      <label className="block">Name<input required disabled={busy || draft.editing} className={`${input} mt-1`} value={draft.name} placeholder="filesystem" onChange={(event) => setDraft({ ...draft, name: event.target.value })} /></label>
      <label className="block">Command<input required disabled={busy} className={`${input} mt-1`} value={draft.command} placeholder="npx" onChange={(event) => setDraft({ ...draft, command: event.target.value })} /></label>
      <label className="block">Arguments (JSON array)<textarea disabled={busy} className={`${input} mt-1 font-mono`} rows={3} value={draft.args} placeholder={'["-y", "@modelcontextprotocol/server-filesystem", "/path/to/folder"]'} onChange={(event) => setDraft({ ...draft, args: event.target.value })} /></label>
      <label className="flex items-center gap-2"><input type="checkbox" disabled={busy} checked={draft.enabled} onChange={(event) => setDraft({ ...draft, enabled: event.target.checked })} />Enable this connection</label>
      <fieldset disabled={busy} className="space-y-2"><legend className="mb-1">Environment variables</legend>
        <p className="text-[12px] text-[var(--color-text-tertiary)]">Stored values are never shown. Adding or removing variables replaces the saved environment.</p>
        {draft.env.map((row, index) => <div className="flex gap-2" key={index}>
          <input aria-label={`Variable ${index + 1} name`} className={input} placeholder="API_KEY" value={row.key} onChange={(event) => setDraft({ ...draft, replaceEnv: true, env: draft.env.map((value, i) => i === index ? { ...value, key: event.target.value } : value) })} />
          <input type="password" autoComplete="off" aria-label={`Variable ${index + 1} value`} className={input} value={row.value} placeholder="Value" onChange={(event) => setDraft({ ...draft, replaceEnv: true, env: draft.env.map((value, i) => i === index ? { ...value, value: event.target.value } : value) })} />
          <button type="button" className={button} aria-label={`Remove variable ${index + 1}`} onClick={() => setDraft({ ...draft, replaceEnv: true, env: draft.env.filter((_, i) => i !== index) })}><Trash2 size={14} /></button>
        </div>)}
        <div className="flex gap-2"><button type="button" className={button} onClick={() => setDraft({ ...draft, replaceEnv: true, env: [...draft.env, { key: "", value: "" }] })}>Add variable</button>
        {draft.editing ? <button type="button" className={button} onClick={() => setDraft({ ...draft, replaceEnv: true, env: [] })}>Clear stored variables</button> : null}</div>
        {draft.replaceEnv ? <p role="status" className="text-[12px]">{draft.env.length ? "Saved variables will be replaced." : "Saved variables will be cleared."}</p> : null}
      </fieldset>
      <div className="flex gap-2"><button className={button} disabled={busy} type="submit">Save connection</button><button className={button} disabled={busy} type="button" onClick={() => setDraft(null)}>Cancel</button></div>
    </form> : null}
    {importText !== null ? <form className="space-y-3 rounded-xl border border-[var(--color-border)] p-4" onSubmit={(event) => { event.preventDefault(); try { void perform({ kind: "import", servers: parseLocalMcpConfig(importText) }, () => setImportText(null)); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); } }}>
      <label className="block">Paste local MCP configuration<textarea aria-label="MCP configuration JSON" disabled={busy} className={`${input} mt-2 font-mono`} rows={6} value={importText} onChange={(event) => setImportText(event.target.value)} placeholder={'{"mcpServers":{"example":{"command":"npx","args":[]}}}'} /></label>
      <p className="text-[12px] text-[var(--color-text-tertiary)]">Imports start disabled. Review the commands, enable the connections you trust, and test them before selecting tools.</p>
      <div className="flex gap-2"><button type="submit" className={button} disabled={busy}>Import local servers</button><button type="button" className={button} disabled={busy} onClick={() => setImportText(null)}>Cancel</button></div>
    </form> : null}
    {state?.servers.length === 0 ? <p className="py-6 text-[var(--color-text-tertiary)]">No local MCP servers yet.</p> : null}
    {state?.servers.map((server) => <section key={server.name} className="space-y-3 rounded-xl border border-[var(--color-border)] bg-[var(--wash-row)] p-4">
      <div className="flex items-center justify-between gap-3"><h3 className="font-medium">{server.name}</h3><label className="flex items-center gap-2 text-[12px]"><input type="checkbox" disabled={busy} checked={server.enabled} onChange={() => void perform({ kind: "save", server: { ...serverInput(server), enabled: !server.enabled } })} />Enabled</label></div>
      <p className="break-all font-mono text-[11px] text-[var(--color-text-tertiary)]">{server.command} {server.args.join(" ")}</p>
      <p className="text-[12px] text-[var(--color-text-secondary)]">{server.status === "tested" ? <><Check size={13} className="mr-1 inline text-green-500" />Tested · {server.tools.length} tools · starts when a selected thread sends a message</> : server.status === "error" ? "Connection failed" : "Not tested"}</p>
      {server.error ? <p role="status" className="text-[12px] text-[var(--color-danger-strong)]">{server.error}</p> : null}
      {server.environmentKeys.length ? <p className="text-[12px] text-[var(--color-text-tertiary)]">Saved variables: {server.environmentKeys.join(", ")}</p> : null}
      <div className="flex flex-wrap gap-2"><button type="button" className={button} disabled={busy} onClick={() => void perform({ kind: "test", name: server.name })}>Test connection</button><button type="button" className={button} disabled={busy} onClick={() => edit(server)}>Edit</button><button type="button" className={button} disabled={busy} onClick={() => void perform({ kind: "delete", name: server.name })}>Remove</button></div>
      {server.tools.length ? <details><summary className="cursor-pointer text-[12px] text-[var(--color-text-secondary)]">Inspect tools</summary><ul className="mt-2 space-y-2">{server.tools.map((tool) => <li key={tool.name}><p className="break-all font-mono text-[11px]">{tool.name}</p><p className="mt-0.5 text-[12px] text-[var(--color-text-tertiary)]">{tool.description}</p></li>)}</ul></details> : null}
    </section>)}
  </div>;
}
