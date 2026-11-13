/** @jsxRuntime automatic */
import { Globe2, LoaderCircle, Plug, Terminal, X } from "lucide-react";
import { useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { AgentControlOptions } from "./AgentModeControl";
import type { DesktopMcpServer } from "@axiom/chat-core/viewTypes";
import { RemoteMcpNotice, mcpHost } from "./McpPrivacy";

export function ToolsControl({ options, open, onOpenChange }: { options: AgentControlOptions; open: boolean; onOpenChange(open: boolean): void }) {
  const trigger = useRef<HTMLButtonElement>(null);
  const count = options.selection.mcpTools?.length ?? 0;
  const remote = options.mcpServers?.some(server => server.remote && server.tools.some(tool => options.selection.mcpTools?.includes(tool.name)));
  const description = `${count} ${count === 1 ? "tool" : "tools"} enabled${remote ? ", including remote tools" : ""}`;
  return <>
    <button ref={trigger} type="button" data-tools-toggle aria-label="Tools" aria-description={description} title={description}
      aria-haspopup="dialog" aria-expanded={open} aria-pressed={count > 0} onClick={() => onOpenChange(true)}
      className={`flex h-7 shrink-0 items-center gap-1.5 rounded-[7px] px-2.5 text-[12px] font-medium transition-colors ${count ? "bg-[var(--wash-chip)] text-[var(--color-text-primary)]" : "text-[var(--color-text-tertiary)] hover:bg-[var(--wash-chip-hover)] hover:text-[var(--color-text-primary)]"}`}>
      {remote ? <Globe2 size={14} aria-hidden="true" /> : <Plug size={14} aria-hidden="true" />}Tools{count ? <span className="text-[10px]">{count}</span> : null}
    </button>
    {open ? createPortal(<ToolsDialog key={options.scope} options={options} onClose={() => { onOpenChange(false); trigger.current?.focus({ preventScroll: true }); }} />, document.body) : null}
  </>;
}

function ToolsDialog({ options, onClose }: { options: AgentControlOptions; onClose(): void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const closeButton = useRef<HTMLButtonElement>(null);
  const live = useRef(false);
  const inFlight = useRef(false);
  const [selected, setSelected] = useState(options.selection.mcpTools ?? []);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const blocked = busy || !!options.disabledReason;
  const remote = options.mcpServers?.filter(server => !!server.remote) ?? [];
  const local = options.mcpServers?.filter(server => !server.remote) ?? [];
  const hasRemote = remote.some(server => server.tools.some(tool => selected.includes(tool.name)));
  useLayoutEffect(() => {
    live.current = true; dialog.current!.showModal(); closeButton.current?.focus();
    return () => { live.current = false; dialog.current?.close(); };
  }, []);
  async function save() {
    if (inFlight.current || blocked) return;
    if (selected.length > 256) { setError("Select up to 256 tools for this conversation."); return; }
    inFlight.current = true; setBusy(true); setError(null);
    try { await options.onApply({ ...options.selection, mcpTools: selected }); if (live.current) onClose(); }
    catch (cause) { if (live.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { inFlight.current = false; if (live.current) setBusy(false); }
  }
  function connection(server: DesktopMcpServer) {
    const host = mcpHost(server.remote?.url);
    return <fieldset key={server.name} disabled={blocked || !server.enabled} className="space-y-2.5 rounded-xl bg-[var(--wash-row)] p-3">
      <legend className="max-w-full px-1 text-[13px] font-medium"><label className="flex items-center gap-2">
        <input type="checkbox" aria-label={`Use ${server.displayName ?? server.name} in this conversation`} disabled={blocked || !server.enabled || !server.tools.length}
          checked={server.tools.length > 0 && server.tools.every(tool => selected.includes(tool.name))}
          ref={element => { if (element) element.indeterminate = server.tools.some(tool => selected.includes(tool.name)) && !server.tools.every(tool => selected.includes(tool.name)); }}
          onChange={event => { const checked = event.target.checked; setSelected(current => checked ? [...new Set([...current, ...server.tools.map(tool => tool.name)])] : current.filter(name => !server.tools.some(tool => tool.name === name))); }} />
        <span className="break-all">{server.displayName ?? server.name}{!server.enabled ? " (disabled)" : ""}</span>
      </label></legend>
      <p className="flex items-center gap-1.5 break-all text-[11px] text-[var(--color-text-tertiary)]">{host ? <Globe2 size={12} className="shrink-0" /> : <Terminal size={12} className="shrink-0" />}{host ? `Remote at ${host}` : "Local process"}</p>
      {host ? <p className="text-[12px] leading-5 text-[var(--color-text-secondary)]">Tool inputs are shared with this service outside Axiom’s attested environment.</p> : null}
      {!server.tools.length ? <p className="text-[12px] text-[var(--color-text-tertiary)]">{server.status === "needs_auth" ? "Sign in to this service in Settings." : "Test this connection in Settings to discover its tools."}</p> : server.tools.map(tool => <label key={tool.name} className="flex items-start gap-2.5 text-[12px]"><input type="checkbox" className="mt-1" checked={selected.includes(tool.name)} onChange={event => setSelected(current => event.target.checked ? [...new Set([...current, tool.name])] : current.filter(name => name !== tool.name))} /><span className="min-w-0"><span className="block break-all font-medium">{tool.name.replace(`mcp__${server.name}__`, "").replaceAll("_", " ")}</span><span className="mt-0.5 block text-[var(--color-text-tertiary)]">{tool.description}</span></span></label>)}
    </fieldset>;
  }
  return <dialog ref={dialog} aria-modal="true" aria-label="Tools for this conversation" aria-describedby="mcp-tools-description"
    onCancel={event => { event.preventDefault(); if (!inFlight.current) onClose(); }}
    onClick={event => { if (event.target !== event.currentTarget || inFlight.current) return; const bounds = event.currentTarget.getBoundingClientRect(); if (event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom) onClose(); }}
    className="no-drag glass-panel-solid shadow-glass-pop m-auto max-h-[calc(100dvh-40px)] w-[520px] max-w-[calc(100%_-_40px)] overflow-y-auto rounded-[22px] border-0 p-0 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)] backdrop:backdrop-blur-[4px]">
    <form onSubmit={event => { event.preventDefault(); void save(); }}>
      <header className="flex items-start justify-between gap-3 px-7 pt-7"><div><h2 className="text-[20px] font-medium tracking-tight">Tools for this conversation</h2><p id="mcp-tools-description" className="mt-1 text-[13px] leading-5 text-[var(--color-text-secondary)]">Choose tools from your connections. Tools work with Agent mode off. Calls ask for approval unless you grant access for this conversation.</p></div><button ref={closeButton} type="button" aria-label="Close tools" disabled={busy} onClick={onClose} className="-mr-1 rounded-lg p-1.5 text-[var(--color-text-tertiary)] hover:bg-[var(--wash-chip-hover)]"><X size={16} /></button></header>
      <div className="space-y-5 px-7 py-6">
        {options.mcpError ? <p role="status" className="text-[12px] text-[var(--color-text-secondary)]">{options.mcpError}</p> : null}
        {!remote.length && !local.length ? <p className="text-[13px] text-[var(--color-text-secondary)]">Add and test connections in Settings → MCP connections.</p> : null}
        {remote.length ? <section aria-label="Remote services" className="space-y-3"><h3 className="text-[13px] font-medium">Remote services</h3>{remote.map(connection)}</section> : null}
        {hasRemote ? <RemoteMcpNotice compact /> : null}
        {local.length ? <section aria-label="On this device" className="space-y-3"><h3 className="text-[13px] font-medium">On this device</h3>{local.map(connection)}<p className="text-[12px] leading-5 text-[var(--color-text-tertiary)]">Local processes run with your computer’s permissions, outside the attested model environment. They can access files and the network.</p></section> : null}
        {error || options.disabledReason ? <p role={error ? "alert" : "status"} className="text-[12px] text-[var(--color-text-secondary)]">{error || options.disabledReason}</p> : null}
      </div>
      <footer className="flex items-center justify-between gap-2 border-t border-[var(--color-border)] px-7 py-4"><span className="text-[12px] text-[var(--color-text-tertiary)]">{selected.length} {selected.length === 1 ? "tool" : "tools"} selected</span><div className="flex gap-2"><button type="button" disabled={busy} onClick={onClose} className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px]">Cancel</button><button type="submit" disabled={blocked} className="flex items-center gap-2 rounded-lg bg-[var(--color-cherry)] px-4 py-2 text-[13px] font-medium text-on-accent disabled:opacity-40">{busy ? <LoaderCircle size={13} className="animate-spin" /> : null}Save tools</button></div></footer>
    </form>
  </dialog>;
}
