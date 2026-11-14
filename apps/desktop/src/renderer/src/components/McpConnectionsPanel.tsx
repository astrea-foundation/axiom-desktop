import { Check, Globe2, LoaderCircle, Plus, RefreshCw, Terminal, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { DesktopMcpAction, DesktopMcpResponse, DesktopMcpServer, DesktopMcpServerInput, McpAuthentication } from "@axiom/axiom-acp-client";
import { RemoteMcpNotice, mcpHost } from "@axiom/chat-ui/McpPrivacy";
import { parseMcpConfig } from "../mcpConfig";

const button = "rounded-lg bg-[var(--wash-chip)] px-3 py-2 text-[12px] transition-colors hover:bg-[var(--wash-chip-hover)] disabled:opacity-40";
const input = "w-full rounded-lg border border-[var(--color-border)] bg-[var(--wash-row)] px-3 py-2 text-[13px] outline-none focus:border-[var(--color-border-accent)] disabled:opacity-50";
type Draft = { key: string; type: "remote" | "local"; name: string; command: string; args: string; enabled: boolean;
  env: Array<{ key: string; value: string }>; replaceEnv: boolean; editing: boolean;
  url: string; auth: McpAuthentication; token: string; replaceToken: boolean; clientId: string; clientMetadataUrl: string };
const emptyDraft = (): Draft => ({ key: "", type: "remote", name: "", command: "", args: "[]", enabled: true, env: [], replaceEnv: false,
  editing: false, url: "", auth: "oauth", token: "", replaceToken: false, clientId: "", clientMetadataUrl: "" });

export function McpConnectionsPanel() {
  const [state, setState] = useState<DesktopMcpResponse | null>(null);
  const stateRef = useRef(state);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [connecting, setConnecting] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [importText, setImportText] = useState<string | null>(null);
  const mounted = useRef(true);
  const inFlight = useRef(false);
  useEffect(() => { mounted.current = true; void refresh(); return () => { mounted.current = false; }; }, []);
  async function request(action: DesktopMcpAction): Promise<DesktopMcpResponse> {
    const api = window.axiomDesktop?.agent;
    if (!api?.desktopMcp) throw new Error("Restart Axiom to load MCP support.");
    const response = await api.desktopMcp({ expectedRevision: stateRef.current?.revision, action });
    if (mounted.current) { stateRef.current = response; setState(response); }
    return response;
  }
  async function perform(work: DesktopMcpAction | (() => Promise<void>), done?: () => void) {
    if (inFlight.current) return;
    inFlight.current = true; setBusy(true); setError(null);
    try { if (typeof work === "function") await work(); else await request(work); if (mounted.current) done?.(); }
    catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { inFlight.current = false; if (mounted.current) { setBusy(false); setConnecting(null); } }
  }
  async function refresh() { await perform({ kind: "list" }); }
  async function signIn(name: string) {
    if (mounted.current) setConnecting(name);
    try { await request({ kind: "connect", name }); }
    finally { if (mounted.current) setConnecting(null); }
  }
  async function cancelSignIn() {
    if (!connecting) return;
    try { await window.axiomDesktop?.agent.desktopMcp({ action: { kind: "cancel_connect", name: connecting } }); }
    catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)); }
  }
  function edit(server: DesktopMcpServer) {
    setImportText(null); setError(null);
    setDraft({ ...emptyDraft(), type: server.remote ? "remote" : "local", key: server.name, name: server.displayName ?? server.name, command: server.command,
      args: JSON.stringify(server.args, null, 2), enabled: server.enabled, editing: true,
      url: server.remote?.url ?? "", auth: server.remote?.auth ?? "none", clientId: server.remote?.clientId ?? "", clientMetadataUrl: server.remote?.clientMetadataUrl ?? "" });
  }
  function serverInput(server: DesktopMcpServer): DesktopMcpServerInput {
    return { name: server.name, displayName: server.displayName, command: server.command, args: server.args, enabled: server.enabled, remote: server.remote };
  }
  function save() {
    if (!draft) return;
    try {
      const baseName = draft.name.trim().toLowerCase().replace(/[^a-z0-9_]/g, "_").replace(/^_+|_+$/g, "").slice(0, 40).replace(/_+$/g, "") || "connection";
      let name = draft.editing ? draft.key : baseName;
      let suffix = 2;
      while (!draft.editing && stateRef.current?.servers.some(server => server.name === name)) name = `${baseName}_${suffix++}`;
      let server: DesktopMcpServerInput;
      if (draft.type === "remote") {
        server = { name, displayName: draft.name.trim(), command: "", args: [], enabled: draft.enabled, remote: {
          url: draft.url.trim(), auth: draft.auth,
          ...(draft.auth === "oauth" && draft.clientId ? { clientId: draft.clientId.trim() } : {}),
          ...(draft.auth === "oauth" && draft.clientMetadataUrl ? { clientMetadataUrl: draft.clientMetadataUrl.trim() } : {}),
        }, ...(draft.auth === "bearer" && draft.replaceToken ? { bearerToken: draft.token } : {}) };
      } else {
        const args: unknown = JSON.parse(draft.args);
        if (!Array.isArray(args) || args.some(arg => typeof arg !== "string")) throw new Error("Arguments must be a JSON array of strings.");
        const env: Record<string, string> = Object.create(null);
        for (const row of draft.env) { if (!row.key || Object.hasOwn(env, row.key)) throw new Error("Environment variable names must be nonempty and unique."); env[row.key] = row.value; }
        server = { name, displayName: draft.name.trim(), command: draft.command, args, enabled: draft.enabled, remote: null, ...(draft.replaceEnv ? { env } : {}) };
      }
      const remote = server.remote;
      void perform(async () => {
        const saved = await request({ kind: "save", server });
        if (remote) {
          const connection = saved.servers.find(s => s.name === server.name);
          if (remote.auth === "oauth" && !connection?.credentialId) await signIn(server.name);
          await request({ kind: "test", name: server.name });
        }
      }, () => setDraft(null));
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
  }
  return <div className="space-y-4 text-[13px]">
    <p className="text-[var(--color-text-secondary)]">Add connections here, then choose their tools under Agent in each conversation.</p>
    <div className="flex flex-wrap gap-2">
      <button type="button" className={button} disabled={busy || !state} onClick={() => { setImportText(null); setDraft({ ...emptyDraft(), type: state?.remoteSupported ? "remote" : "local" }); setError(null); }}><Plus size={13} className="mr-1 inline" />Add connection</button>
      <button type="button" className={button} disabled={busy || !state} onClick={() => { setDraft(null); setImportText(""); setError(null); }}>Import configuration</button>
      <button type="button" className={button} disabled={busy} onClick={() => void refresh()} aria-label="Refresh MCP connections">{busy ? <LoaderCircle size={14} className="animate-spin" /> : <RefreshCw size={14} />}</button>
    </div>
    {error ? <p role="alert" className="text-[var(--color-danger-strong)]">{error}</p> : null}
    {connecting ? <div role="status" className="flex flex-wrap items-center gap-3 rounded-xl bg-[var(--wash-chip)] p-3"><LoaderCircle size={14} className="animate-spin" /><span>Finish signing in to {connecting} in your browser. Sign-in expires after 3 minutes.</span><button type="button" className={button} onClick={() => void cancelSignIn()}>Cancel sign-in</button></div> : null}
    {draft ? <form className="space-y-4 rounded-xl border border-[var(--color-border)] p-4" onSubmit={event => { event.preventDefault(); save(); }}>
      <h3 className="font-medium">{draft.editing ? "Edit connection" : "Add connection"}</h3>
      <fieldset disabled={busy || draft.editing} className="flex gap-2" aria-label="Connection type">{([['remote', 'Remote service', Globe2], ['local', 'Local process', Terminal]] as const).map(([type, label, Icon]) => <label key={type} className={`flex flex-1 cursor-pointer items-center gap-2 rounded-lg border px-3 py-3 ${draft.type === type ? "border-[var(--color-border-accent)] bg-[var(--wash-chip)]" : "border-[var(--color-border)]"}`}><input type="radio" name="mcp-type" value={type} disabled={type === "remote" && !state?.remoteSupported} checked={draft.type === type} onChange={() => setDraft({ ...draft, type })} /><Icon size={14} />{label}</label>)}</fieldset>
      {!state?.remoteSupported ? <p className="text-[12px] text-[var(--color-text-tertiary)]">Restart Axiom with the updated native app to add remote services.</p> : null}
      <label className="block">Name<input aria-label="Name" required disabled={busy} maxLength={128} className={`${input} mt-1`} value={draft.name} placeholder={draft.type === "remote" ? "Project tracker" : "Local files"} onChange={event => setDraft({ ...draft, name: event.target.value })} /></label>
      {draft.type === "remote" ? <>
        <label className="block">Service URL<input required type="url" disabled={busy} className={`${input} mt-1`} value={draft.url} placeholder="https://mcp.example.com/mcp" onChange={event => setDraft({ ...draft, url: event.target.value })} /></label>
        <label className="block">Authentication<select aria-label="Authentication" disabled={busy} className={`${input} mt-1`} value={draft.auth} onChange={event => setDraft({ ...draft, auth: event.target.value as McpAuthentication })}><option value="oauth">Sign in with browser</option><option value="bearer">API token</option><option value="none">No authentication</option></select></label>
        {draft.auth === "bearer" ? <label className="block">API token<input aria-label="API token" type="password" autoComplete="off" disabled={busy} className={`${input} mt-1`} value={draft.token} placeholder={draft.editing ? "Leave unchanged to keep the saved token" : "Token"} onChange={event => setDraft({ ...draft, token: event.target.value, replaceToken: true })} /><span className="mt-1 block text-[11px] text-[var(--color-text-tertiary)]">Stored in your system credential store. Saved tokens are never shown.</span>{draft.editing ? <button type="button" disabled={busy} className="mt-1 underline" onClick={() => setDraft({ ...draft, token: "", replaceToken: true })}>Clear saved token</button> : null}</label> : null}
        {draft.auth === "oauth" ? <details><summary className="cursor-pointer text-[12px] text-[var(--color-text-secondary)]">Advanced sign-in settings</summary><div className="mt-3 space-y-3"><label className="block">Registered client ID (optional)<input disabled={busy} className={`${input} mt-1`} value={draft.clientId} onChange={event => setDraft({ ...draft, clientId: event.target.value })} /></label><label className="block">Client metadata document URL (optional)<input type="url" disabled={busy} className={`${input} mt-1`} value={draft.clientMetadataUrl} onChange={event => setDraft({ ...draft, clientMetadataUrl: event.target.value })} /></label></div></details> : null}
        <RemoteMcpNotice destination={draft.url} />
      </> : <>
        <label className="block">Command<input required disabled={busy} className={`${input} mt-1`} value={draft.command} placeholder="npx" onChange={event => setDraft({ ...draft, command: event.target.value })} /></label>
        <label className="block">Arguments (JSON array)<textarea disabled={busy} className={`${input} mt-1 font-mono`} rows={3} value={draft.args} onChange={event => setDraft({ ...draft, args: event.target.value })} /></label>
        <fieldset disabled={busy} className="space-y-2"><legend className="mb-1">Environment variables</legend><p className="text-[12px] text-[var(--color-text-tertiary)]">Stored values are never shown. Adding or removing variables replaces the saved environment.</p>
          {draft.env.map((row, index) => <div className="flex gap-2" key={index}><input aria-label={`Variable ${index + 1} name`} className={input} placeholder="API_KEY" value={row.key} onChange={event => setDraft({ ...draft, replaceEnv: true, env: draft.env.map((v, i) => i === index ? { ...v, key: event.target.value } : v) })} /><input type="password" autoComplete="off" aria-label={`Variable ${index + 1} value`} className={input} value={row.value} placeholder="Value" onChange={event => setDraft({ ...draft, replaceEnv: true, env: draft.env.map((v, i) => i === index ? { ...v, value: event.target.value } : v) })} /><button type="button" className={button} aria-label={`Remove variable ${index + 1}`} onClick={() => setDraft({ ...draft, replaceEnv: true, env: draft.env.filter((_, i) => i !== index) })}><Trash2 size={14} /></button></div>)}
          <div className="flex gap-2"><button type="button" className={button} onClick={() => setDraft({ ...draft, replaceEnv: true, env: [...draft.env, { key: "", value: "" }] })}>Add variable</button>{draft.editing ? <button type="button" className={button} onClick={() => setDraft({ ...draft, replaceEnv: true, env: [] })}>Clear stored variables</button> : null}</div>
          {draft.replaceEnv ? <p role="status" className="text-[12px]">{draft.env.length ? "Saved variables will be replaced." : "Saved variables will be cleared."}</p> : null}
        </fieldset>
        <p className="rounded-xl bg-[var(--wash-chip)] p-3 text-[12px] leading-5 text-[var(--color-text-secondary)]">Local processes run with your computer’s permissions and can access files and the network. They run outside the attested model environment. Only add programs you trust.</p>
      </>}
      <label className="flex items-center gap-2"><input type="checkbox" disabled={busy} checked={draft.enabled} onChange={event => setDraft({ ...draft, enabled: event.target.checked })} />Enable this connection</label>
      <div className="flex gap-2"><button className={button} disabled={busy} type="submit">{busy ? "Connecting…" : draft.type === "remote" ? "Connect" : "Save connection"}</button><button className={button} disabled={busy} type="button" onClick={() => setDraft(null)}>Cancel</button></div>
    </form> : null}
    {importText !== null ? <form className="space-y-3 rounded-xl border border-[var(--color-border)] p-4" onSubmit={event => { event.preventDefault(); try { void perform({ kind: "import", servers: parseMcpConfig(importText) }, () => setImportText(null)); } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); } }}>
      <label className="block">Paste MCP configuration<textarea aria-label="MCP configuration JSON" disabled={busy} className={`${input} mt-2 font-mono`} rows={6} value={importText} onChange={event => setImportText(event.target.value)} placeholder={'{"mcpServers":{"example":{"url":"https://mcp.example.com/mcp"}}}'} /></label>
      <p className="text-[12px] text-[var(--color-text-tertiary)]">Imports start disabled. Review each destination or command, enable the connections you trust, and test them before selecting tools.</p>
      <RemoteMcpNotice compact />
      <div className="flex gap-2"><button type="submit" className={button} disabled={busy}>Import connections</button><button type="button" className={button} disabled={busy} onClick={() => setImportText(null)}>Cancel</button></div>
    </form> : null}
    {state?.servers.length === 0 ? <p className="py-6 text-[var(--color-text-tertiary)]">No MCP connections yet.</p> : null}
    {state?.servers.map(server => <section key={server.name} aria-label={`${server.displayName ?? server.name} connection`} className="space-y-3 rounded-xl border border-[var(--color-border)] bg-[var(--wash-row)] p-4">
      <div className="flex items-center justify-between gap-3"><div className="flex items-center gap-2"><h3 className="font-medium">{server.displayName ?? server.name}</h3><span className="flex items-center gap-1 rounded-md bg-[var(--wash-chip)] px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)]">{server.remote ? <Globe2 size={11} /> : <Terminal size={11} />}{server.remote ? "Remote" : "Local process"}</span></div><label className="flex items-center gap-2 text-[12px]"><input type="checkbox" disabled={busy} checked={server.enabled} onChange={() => void perform({ kind: "save", server: { ...serverInput(server), enabled: !server.enabled } })} />Enabled</label></div>
      <p className="break-all font-mono text-[11px] text-[var(--color-text-tertiary)]">{server.remote ? mcpHost(server.remote.url) : `${server.command} ${server.args.join(" ")}`}</p>
      <p className="text-[12px] text-[var(--color-text-secondary)]">{server.status === "tested" ? <><Check size={13} className="mr-1 inline" />Ready with {server.tools.length} {server.tools.length === 1 ? "tool" : "tools"}</> : server.status === "needs_auth" ? "Needs sign-in or API token" : server.status === "error" ? "Connection failed" : "Not tested"}</p>
      {server.error ? <p role="status" className="text-[12px] text-[var(--color-danger-strong)]">{server.error}</p> : null}
      {server.remote ? <p className="text-[12px] leading-5 text-[var(--color-text-secondary)]">Tool inputs are shared with this service outside Axiom’s attested environment.</p> : <p className="text-[12px] leading-5 text-[var(--color-text-tertiary)]">Runs with your computer’s permissions, outside the attested model environment.</p>}
      {server.environmentKeys.length ? <p className="text-[12px] text-[var(--color-text-tertiary)]">Saved variables: {server.environmentKeys.join(", ")}</p> : null}
      <div className="flex flex-wrap gap-2"><button type="button" className={button} disabled={busy} onClick={() => void perform({ kind: "test", name: server.name })}>Test connection</button>{server.remote?.auth === "oauth" ? <button type="button" className={button} disabled={busy} onClick={() => void perform(async () => { await signIn(server.name); await request({ kind: "test", name: server.name }); })}>{server.credentialId ? "Reconnect" : "Sign in"}</button> : null}<button type="button" className={button} disabled={busy} onClick={() => edit(server)}>Edit</button>{server.remote && server.credentialId ? <button type="button" className={button} disabled={busy} onClick={() => void perform({ kind: "disconnect", name: server.name })}>Disconnect</button> : null}<button type="button" className={button} disabled={busy} onClick={() => void perform({ kind: "delete", name: server.name })}>Remove</button></div>
      {server.tools.length ? <details><summary className="cursor-pointer text-[12px] text-[var(--color-text-secondary)]">Inspect tools</summary><ul className="mt-2 space-y-2">{server.tools.map(tool => <li key={tool.name}><p className="break-all font-mono text-[11px]">{tool.name}</p><p className="mt-0.5 text-[12px] text-[var(--color-text-tertiary)]">{tool.description}</p></li>)}</ul></details> : null}
    </section>)}
  </div>;
}
