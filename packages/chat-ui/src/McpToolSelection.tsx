/** @jsxRuntime automatic */
import { Globe2, Terminal } from "lucide-react";
import type { DesktopMcpServer } from "@axiom/chat-core/viewTypes";
import { RemoteMcpNotice, mcpHost } from "./McpPrivacy";

export function McpToolSelection({ servers = [], selected, disabled, error, onChange }: {
  servers?: DesktopMcpServer[];
  selected: string[];
  disabled: boolean;
  error?: string;
  onChange(selected: string[]): void;
}) {
  const remote = servers.filter(server => !!server.remote);
  const local = servers.filter(server => !server.remote);
  const hasRemote = remote.some(server => server.tools.some(tool => selected.includes(tool.name)));
  function connection(server: DesktopMcpServer) {
    const host = mcpHost(server.remote?.url);
    return <fieldset key={server.name} disabled={disabled || !server.enabled} className="space-y-2.5 rounded-xl bg-[var(--wash-row)] p-3">
      <legend className="max-w-full px-1 text-[13px] font-medium"><label className="flex items-center gap-2">
        <input type="checkbox" aria-label={`Use ${server.displayName ?? server.name} in this conversation`} disabled={disabled || !server.enabled || !server.tools.length}
          checked={server.tools.length > 0 && server.tools.every(tool => selected.includes(tool.name))}
          ref={element => { if (element) element.indeterminate = server.tools.some(tool => selected.includes(tool.name)) && !server.tools.every(tool => selected.includes(tool.name)); }}
          onChange={event => onChange(event.target.checked ? [...new Set([...selected, ...server.tools.map(tool => tool.name)])] : selected.filter(name => !server.tools.some(tool => tool.name === name)))} />
        <span className="break-all">{server.displayName ?? server.name}{!server.enabled ? " · disabled" : ""}</span>
      </label></legend>
      <p className="flex items-center gap-1.5 break-all text-[11px] text-[var(--color-text-tertiary)]">{host ? <Globe2 size={12} className="shrink-0" /> : <Terminal size={12} className="shrink-0" />}{host ? `Remote · ${host}` : "Local process"}</p>
      {host ? <p className="text-[12px] leading-5 text-[var(--color-text-secondary)]">Tool inputs are shared with this service outside Axiom’s attested environment.</p> : null}
      {!server.tools.length ? <p className="text-[12px] text-[var(--color-text-tertiary)]">{server.status === "needs_auth" ? "Sign in to this service in Settings." : "Test this connection in Settings to discover its tools."}</p> : server.tools.map(tool => <label key={tool.name} className="flex items-start gap-2.5 text-[12px]"><input type="checkbox" className="mt-1" checked={selected.includes(tool.name)} onChange={event => onChange(event.target.checked ? [...new Set([...selected, tool.name])] : selected.filter(name => name !== tool.name))} /><span className="min-w-0"><span className="block break-all font-medium">{tool.name.replace(`mcp__${server.name}__`, "").replaceAll("_", " ")}</span><span className="mt-0.5 block text-[var(--color-text-tertiary)]">{tool.description}</span></span></label>)}
    </fieldset>;
  }
  return <section aria-label="MCP connections" className="space-y-3 border-t border-[var(--color-border)] pt-5">
    <div><h3 className="text-[13px] font-medium text-[var(--color-text-secondary)]">MCP connections</h3><p className="mt-1 text-[12px] leading-5 text-[var(--color-text-tertiary)]">Choose tools for this thread. Connections work with Agent mode off. Calls ask for approval unless you grant access for this conversation.</p></div>
    {error ? <p role="status" className="text-[12px] text-[var(--color-text-secondary)]">{error}</p> : null}
    {!servers.length ? <p className="text-[12px] text-[var(--color-text-tertiary)]">Add and test connections in Settings → MCP connections.</p> : null}
    {remote.length ? <section aria-label="Remote services" className="space-y-3"><h4 className="text-[12px] font-medium">Remote services</h4>{remote.map(connection)}</section> : null}
    {hasRemote ? <RemoteMcpNotice compact /> : null}
    {local.length ? <section aria-label="On this device" className="space-y-3"><h4 className="text-[12px] font-medium">On this device</h4>{local.map(connection)}<p className="text-[12px] leading-5 text-[var(--color-text-tertiary)]">Local processes run with your computer’s permissions, outside the attested model environment. They can access files and the network.</p></section> : null}
    {servers.length || selected.length ? <p className="text-[12px] text-[var(--color-text-tertiary)]">{selected.length} {selected.length === 1 ? "tool" : "tools"} selected</p> : null}
  </section>;
}
