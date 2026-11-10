import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Check, Copy, KeyRound, Plus, RefreshCw } from "lucide-react";
import type { ApiKeyRecord } from "@axiom/axiom-acp-client";
import { desktopErrorMessage } from "../signInFlow";

export function ApiKeysPanel({accountId, onLogin}: {accountId: string; onLogin: () => void}) {
  const [keys, setKeys] = useState<ApiKeyRecord[] | null>(null);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [secret, setSecret] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [reauth, setReauth] = useState(false);
  const [revoking, setRevoking] = useState<ApiKeyRecord | null>(null);
  const [history, setHistory] = useState(false);
  const alive = useRef(false), pending = useRef(false), loadVersion = useRef(0);
  const createButton = useRef<HTMLButtonElement>(null);
  const refresh = useCallback(async () => {
    const version = ++loadVersion.current;
    try {
      const next = (await window.axiomDesktop!.agent.apiKeys(accountId)).keys;
      if (alive.current && version === loadVersion.current) { setKeys(next); setError(null); }
    } catch (cause) {
      if (alive.current && version === loadVersion.current) setError(desktopErrorMessage(cause, "API keys could not be loaded. Please try again."));
    }
  }, [accountId]);
  useEffect(() => {
    alive.current = true;
    void refresh();
    const focus = () => { if (!pending.current) void refresh(); };
    const hide = () => { setSecret(null); };
    window.addEventListener("focus", focus); window.addEventListener("pagehide", hide);
    return () => { alive.current = false; loadVersion.current++; window.removeEventListener("focus", focus); window.removeEventListener("pagehide", hide); };
  }, [refresh]);
  async function create() {
    if (pending.current || !name.trim()) return;
    pending.current = true; loadVersion.current++; setBusy(true); setError(null); setReauth(false);
    try {
      const result = await window.axiomDesktop!.agent.createApiKey(name, accountId);
      if (!alive.current) return;
      const {token, key} = result;
      setKeys(current => [key, ...(current ?? [])]);
      setSecret(token); setCopied(false); setCreating(false); setName("");
      void refresh();
    } catch (cause) {
      if (!alive.current) return;
      const needsAuth = desktopErrorMessage(cause, "").includes("Sign in again");
      setReauth(needsAuth);
      setError(needsAuth ? "Sign in again before creating an API key." : desktopErrorMessage(cause, "API keys could not be loaded. Please try again."));
    } finally { pending.current = false; if (alive.current) setBusy(false); }
  }
  async function revoke() {
    if (pending.current || !revoking) return;
    pending.current = true; loadVersion.current++; setBusy(true); setError(null);
    try {
      await window.axiomDesktop!.agent.revokeApiKey(revoking.id, accountId);
      if (!alive.current) return;
      setKeys(current => current?.map(key => key.id === revoking.id ? {...key, revokedAt: new Date().toISOString()} : key) ?? null);
      setRevoking(null);
      await refresh();
    } catch (cause) { if (alive.current) setError(desktopErrorMessage(cause, "API keys could not be loaded. Please try again.")); }
    finally { pending.current = false; if (alive.current) setBusy(false); }
  }
  async function copy() {
    if (!secret) return;
    try { await navigator.clipboard.writeText(secret); if (alive.current) setCopied(true); }
    catch { if (alive.current) setError("Couldn’t copy the key. Select it and copy it manually."); }
  }
  function closeCreation() { setCreating(false); setSecret(null); setName(""); setReauth(false); createButton.current?.focus(); }
  const inactive = (key: ApiKeyRecord) => Boolean(key.revokedAt || (key.expiresAt && Date.parse(key.expiresAt) <= Date.now()));
  const visible = keys?.filter(key => history || !inactive(key));
  return <div className="desktop-api-keys api-keys-card">
    <section id="api-keys" aria-label="API keys">
      <div className="section-heading"><div className="key-heading">Your keys</div>
        <div className="key-actions"><button type="button" className="key-refresh" aria-label="Refresh API keys" disabled={busy} onClick={() => void refresh()}><RefreshCw size={15} /></button>
          <button ref={createButton} type="button" className="action-button" disabled={creating || secret !== null || busy} onClick={() => {setCreating(true); setError(null);}}><Plus size={14} />Create key</button></div></div>
      {error && <Notice kind="error">{error} {reauth && <button type="button" className="quiet-link" onClick={onLogin}>Sign in again</button>}</Notice>}
      {creating && <form className="key-create-form" aria-label="Create API key" onSubmit={event => {event.preventDefault(); void create();}}>
        <label className="field"><span>Key name</span><input autoFocus required maxLength={120} autoComplete="off" placeholder="e.g. My coding tools" value={name} disabled={busy} onChange={event => setName(event.target.value)} /></label>
        <div className="key-actions"><button className="action-button" disabled={busy || !name.trim()}>{busy && <Spinner />}Create key</button><button type="button" className="text-button" disabled={busy} onClick={closeCreation}>Cancel</button></div>
      </form>}
      {secret && <div className="key-secret" role="region" aria-label="New API key"><strong>Your key is ready.</strong><p>Copy it now. It won’t be shown again.</p>
        <label className="field"><span className="sr-only">New API key</span><input readOnly autoFocus autoComplete="off" spellCheck={false} value={secret} onFocus={event => event.target.select()} /></label>
        <div className="key-actions"><button type="button" className="action-button" onClick={() => void copy()}>{copied ? <Check size={14} /> : <Copy size={14} />}{copied ? "Copied" : "Copy key"}</button><button type="button" className="text-button" onClick={closeCreation}>Done</button></div>
      </div>}
      {!keys && !error && <div className="center-loading"><Spinner />Loading keys</div>}
      {visible?.length === 0 && <div className="key-empty"><KeyRound size={22} /><h3>{keys?.length ? "No active API keys" : "No API keys yet"}</h3><p>Create a key to connect your tools to Axiom.</p></div>}
      <div className="key-list">{visible?.map(key => <article className="key-row" key={key.id} aria-label={`API key ${key.name}`}>
        <div className="key-summary"><div className="key-name"><strong>{key.name}</strong><span>{key.revokedAt ? "Revoked" : inactive(key) ? "Expired" : key.lastUsedAt ? `Last used ${date(key.lastUsedAt)}` : "Not used yet"}</span></div>
          <div className="key-stat"><strong title={exactCost(key.usage.costMicrousd)}>{cost(key.usage.costMicrousd)}</strong><span>spent</span></div>
          <div className="key-stat"><strong>{count(key.usage.requestCount)}</strong><span>requests</span></div>
          {!inactive(key) && <button type="button" className="quiet-link" disabled={busy} onClick={() => {setError(null); setRevoking(key);}}>Revoke</button>}</div>
        <details className="key-details"><summary>Usage details</summary><dl><div><dt>Input tokens</dt><dd>{count(key.usage.inputTokens)}</dd></div><div><dt>Cached input</dt><dd>{count(key.usage.cachedInputTokens)}</dd></div><div><dt>Output tokens</dt><dd>{count(key.usage.outputTokens)}</dd></div><div><dt>Exact spend</dt><dd>{exactCost(key.usage.costMicrousd)}</dd></div></dl><p>Tracked since {date(key.usageStartedAt)}. Spend reflects posted charges and refunds; tokens reflect reported usage.</p></details>
      </article>)}</div>
      {keys?.some(inactive) && <button type="button" className="quiet-link key-history" aria-expanded={history} onClick={() => setHistory(value => !value)}>{history ? "Hide inactive keys" : "Show revoked and expired keys"}</button>}
      {revoking && <ConfirmDialog title={`Revoke “${revoking.name}”?`} confirmLabel="Revoke key" busy={busy} onCancel={() => {setRevoking(null); setError(null);}} onConfirm={() => void revoke()}><p>Tools using this key will stop connecting. Its usage history stays available.</p>{error && <Notice kind="error">{error}</Notice>}</ConfirmDialog>}
    </section>
  </div>;
}
function date(value: string) { return new Date(value).toLocaleDateString(undefined, {month: "short", day: "numeric", year: "numeric"}); }
function count(value: string) { return BigInt(value).toLocaleString(); }
function exactCost(value: string) { const n = BigInt(value); return `$${(n / 1_000_000n).toLocaleString("en-US")}.${(n % 1_000_000n).toString().padStart(6, "0")}`; }
function cost(value: string) { const n = BigInt(value); if (n > 0n && n < 10_000n) return "<$0.01"; const cents = (n + 5_000n) / 10_000n; return `$${(cents / 100n).toLocaleString("en-US")}.${(cents % 100n).toString().padStart(2, "0")}`; }

function Spinner() { return <RefreshCw size={14} className="animate-spin motion-reduce:animate-none" />; }
function Notice({children}: {kind: "error"; children: ReactNode}) { return <div className="key-notice" role="alert">{children}</div>; }
function ConfirmDialog({title, confirmLabel, busy, onCancel, onConfirm, children}: {
  title: string; confirmLabel: string; busy: boolean; onCancel: () => void; onConfirm: () => void; children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null), cancel = useRef<HTMLButtonElement>(null);
  const id = useId();
  useLayoutEffect(() => {
    const element = ref.current!;
    const opener = document.activeElement as HTMLElement | null;
    element.showModal(); cancel.current?.focus();
    return () => { element.close(); if (opener?.isConnected) opener.focus({preventScroll: true}); };
  }, []);
  return <dialog ref={ref} aria-modal="true" aria-labelledby={id} onCancel={event => {event.preventDefault(); if (!busy) onCancel();}}
    className="no-drag glass-panel-solid shadow-glass-pop m-auto w-[460px] max-w-[calc(100%_-_40px)] rounded-[22px] border-0 p-7 text-[var(--color-text-primary)] backdrop:bg-[var(--surface-scrim)]">
    <h2 id={id} className="mb-4 text-[20px] font-medium">{title}</h2><div className="text-[13px] leading-6 text-[var(--color-text-secondary)]">{children}</div>
    <div className="mt-6 flex justify-end gap-2"><button ref={cancel} type="button" disabled={busy} onClick={onCancel} className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] disabled:opacity-50">Cancel</button><button type="button" disabled={busy} onClick={onConfirm} className="rounded-lg bg-[var(--wash-chip)] px-4 py-2 text-[13px] disabled:opacity-50">{busy ? "Revoking…" : confirmLabel}</button></div>
  </dialog>;
}
