/** @jsxRuntime automatic */
import { useEffect, useRef, useState } from 'react';
import { Copy, Download, LockKeyhole, ShieldCheck } from 'lucide-react';

type Mode = 'setup' | 'unlock' | 'recover' | 'change-password' | 'reset';
export interface VaultScreenProps {
  initialMode: 'setup' | 'unlock';
  accountLabel: string;
  onUnlock(password: string): Promise<void>;
  onRecover(code: string): Promise<void>;
  onPrepare(password: string): Promise<string>;
  onCommit(): Promise<void>;
  onResetChallenge(): Promise<{ wait_seconds: number }>;
  onReset(): Promise<void>;
  onCancelPreparation(): void;
  onSignOut(): void;
  onDownloadUnsynced?(): Promise<void>;
  onUseSynced?(): Promise<void>;
}
const inputClass = 'w-full rounded-[10px] border border-[var(--color-border)] bg-[var(--wash-row)] px-3 py-2.5 text-[14px] outline-none focus:border-[var(--color-border-accent)]';
const buttonClass = 'rounded-[10px] bg-[var(--color-cherry)] px-4 py-2.5 text-[13px] font-medium text-on-accent hover:bg-[var(--color-cherry-bright)] disabled:opacity-40';

/** Secrets stay in transient form state and are cleared before callbacks settle. */
export function VaultScreen(props: VaultScreenProps) {
  const [mode, setMode] = useState<Mode>(props.initialMode);
  const [password, setPassword] = useState(''), [confirmation, setConfirmation] = useState('');
  const [recovery, setRecovery] = useState(''), [code, setCode] = useState('');
  const [acknowledged, setAcknowledged] = useState(false), [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [remaining, setRemaining] = useState<number | null>(null), [phrase, setPhrase] = useState('');
  const [backupSaved, setBackupSaved] = useState(false), [discardConfirmed, setDiscardConfirmed] = useState(false);
  const lifetime = useRef(0);
  useEffect(() => () => { lifetime.current++; }, []);
  const perform = async (action: () => Promise<void>) => {
    const scope = lifetime.current;
    setError(''); setBusy(true);
    try { await action(); }
    catch (failure) { if (scope === lifetime.current) setError(failure instanceof Error ? failure.message : 'Couldn’t continue. Try again.'); }
    finally { if (scope === lifetime.current) setBusy(false); }
  };
  useEffect(() => {
    if (mode !== 'reset') return;
    let cancelled = false, timer: ReturnType<typeof setInterval> | undefined;
    setRemaining(null);
    void props.onResetChallenge().then(({ wait_seconds }) => {
      if (cancelled) return;
      let left = Math.max(20000, wait_seconds * 1000), last = performance.now();
      setRemaining(Math.ceil(left / 1000));
      timer = setInterval(() => {
        const now = performance.now();
        if (!document.hidden) left = Math.max(0, left - Math.min(now - last, 1000));
        last = now; setRemaining(Math.ceil(left / 1000));
      }, 250);
    }).catch(() => { if (!cancelled) setError('Sign in again to confirm deletion.'); });
    return () => { cancelled = true; if (timer) clearInterval(timer); };
  }, [mode]);
  const cancel = () => {
    lifetime.current++; props.onCancelPreparation();
    setPassword(''); setConfirmation(''); setRecovery(''); setCode(''); setPhrase('');
    setAcknowledged(false); setSaved(false); setRemaining(null); setBusy(false); setError(''); setMode(props.initialMode);
  };
  const newPassword = mode === 'setup' || mode === 'change-password';
  const title = code ? 'Save your recovery code' : mode === 'setup' ? 'Create an encryption password'
    : mode === 'unlock' ? 'Unlock your chats' : mode === 'recover' ? 'Use your recovery code'
    : mode === 'change-password' ? 'Choose a new encryption password' : 'Delete all web chats?';
  const download = () => {
    const url = URL.createObjectURL(new Blob([code + '\n'], { type: 'text/plain;charset=utf-8' }));
    const link = document.createElement('a'); link.href = url; link.download = 'axiom-recovery-code.txt'; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return <div className="flex h-full min-h-0 items-center justify-center overflow-y-auto px-5 py-8">
    <section className="glass-panel-solid shadow-glass-card w-full max-w-[440px] rounded-[22px] border border-[var(--color-border)] p-6 sm:p-8" aria-labelledby="vault-title">
      <LockKeyhole size={24} className="mb-5 text-[var(--color-cherry)]" aria-hidden="true" />
      <h1 id="vault-title" className="text-[23px] font-medium tracking-tight">{title}</h1>
      <p className="mt-1 text-[12px] text-[var(--color-text-tertiary)]">{props.accountLabel}</p>
      {code ? <div className="mt-5 space-y-4">
        <p className="text-[13px] leading-6 text-[var(--color-text-secondary)]">Keep this code somewhere safe. It can unlock your chats if you forget your password.</p>
        <div className="flex flex-wrap gap-2">
          <button type="button" className={buttonClass + ' flex items-center gap-2'} onClick={download}><Download size={14} />Download recovery code</button>
          <button type="button" className={inputClass + ' !w-auto flex items-center gap-2'} onClick={() => void perform(async () => { await navigator.clipboard.writeText(code); })}><Copy size={14} />Copy</button>
        </div>
        <label className="flex items-start gap-2 text-[13px]"><input type="checkbox" checked={saved} onChange={event => setSaved(event.target.checked)} className="mt-1" />I saved my recovery code.</label>
        <button type="button" disabled={!saved || busy} className={buttonClass + ' w-full'} onClick={() => void perform(props.onCommit)}>{busy ? 'Saving…' : mode === 'setup' ? 'Open chats' : 'Save new password'}</button>
      </div> : mode === 'reset' ? <div className="mt-5 space-y-4">
        <p className="text-[13px] leading-6 text-[var(--color-text-secondary)]">This permanently deletes all your saved web chats and files on every device. Axiom cannot restore them. Your account and balance will stay.</p>
        <label className="block text-[12px]">Type DELETE ALL CHATS<input aria-label="Confirm deletion" autoComplete="off" value={phrase} onChange={event => setPhrase(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') event.preventDefault(); }} className={inputClass + ' mt-2'} /></label>
        <label className="flex items-start gap-2 text-[13px]"><input type="checkbox" checked={acknowledged} onChange={event => setAcknowledged(event.target.checked)} className="mt-1" />I understand this deletes my web chats and files.</label>
        <button type="button" disabled={remaining !== 0 || phrase !== 'DELETE ALL CHATS' || !acknowledged || busy} className={buttonClass + ' w-full !bg-[var(--color-error)]'} onClick={() => void perform(props.onReset)}>{busy ? 'Deleting…' : remaining === null ? 'Preparing confirmation…' : remaining > 0 ? `Delete all chats (${remaining}s)` : 'Delete all chats and start fresh'}</button>
      </div> : <form className="mt-5 space-y-4" onSubmit={event => {
        event.preventDefault();
        if (busy) return;
        if (newPassword && (password !== confirmation || password.length < 12 || !acknowledged)) return;
        const input = mode === 'recover' ? recovery : password;
        setPassword(''); setConfirmation(''); setRecovery('');
        void perform(async () => {
          if (mode === 'unlock') await props.onUnlock(input);
          else if (mode === 'recover') { const scope = lifetime.current; await props.onRecover(input); if (scope === lifetime.current) { setMode('change-password'); setAcknowledged(false); } }
          else { const scope = lifetime.current, next = await props.onPrepare(input); if (scope === lifetime.current) setCode(next); else props.onCancelPreparation(); }
        });
      }}>
        {mode === 'recover' ? <>
          <label className="block text-[12px]">Recovery code<textarea aria-label="Recovery code" autoComplete="off" spellCheck={false} value={recovery} onChange={event => setRecovery(event.target.value)} className={inputClass + ' mt-2 min-h-24 resize-y font-mono text-[12px]'} maxLength={256} /></label>
          <label className="inline-block cursor-pointer text-[12px] underline">Choose recovery file<input type="file" accept="text/plain,.txt" className="hidden" onChange={event => {
            const file = event.target.files?.[0]; event.target.value = '';
            if (!file) return;
            if (file.size > 1024) { setError('Invalid recovery file'); return; }
            const scope = lifetime.current; void file.text().then(value => { if (scope === lifetime.current) setRecovery(value.trim()); }).catch(() => { if (scope === lifetime.current) setError('Couldn’t read this recovery file'); });
          }} /></label>
        </> : <>
          <label className="block text-[12px]">Encryption password<input type="password" autoFocus autoComplete={newPassword ? 'new-password' : 'current-password'} aria-label="Encryption password" value={password} onChange={event => setPassword(event.target.value)} minLength={newPassword ? 12 : undefined} maxLength={1024} className={inputClass + ' mt-2'} required /></label>
          {newPassword && <label className="block text-[12px]">Confirm password<input type="password" autoComplete="new-password" aria-label="Confirm encryption password" value={confirmation} onChange={event => setConfirmation(event.target.value)} maxLength={1024} className={inputClass + ' mt-2'} required /></label>}
        </>}
        {newPassword && <>
          <p className="text-[12px] leading-5 text-[var(--color-text-secondary)]">Axiom cannot recover your encryption password. Save your recovery code. Without either, your chats cannot be decrypted.</p>
          <label className="flex items-start gap-2 text-[12px]"><input type="checkbox" checked={acknowledged} onChange={event => setAcknowledged(event.target.checked)} className="mt-0.5" />I understand and will save my recovery code.</label>
          {confirmation && password !== confirmation && <p className="text-[12px] text-[var(--color-error)]">Passwords don’t match.</p>}
        </>}
        <button type="submit" disabled={busy || (mode === 'recover' ? !recovery.trim() : !password) || (newPassword && (password.length < 12 || password !== confirmation || !acknowledged))} className={buttonClass + ' w-full'}>{busy ? 'Working…' : mode === 'unlock' ? 'Unlock' : mode === 'recover' ? 'Recover chats' : 'Continue'}</button>
      </form>}
      {error && <p role="alert" className="mt-4 text-[12px] text-[var(--color-error)]">{error} {error.includes('Sign in again') && <a href="/signin?reauth=1&return_to=%2Fchat%2F" className="underline">Sign in</a>}</p>}
      {error.includes('encrypted local copy was kept') && props.onDownloadUnsynced && props.onUseSynced && <div className="mt-4 space-y-3 text-[12px]">
        <button type="button" disabled={busy} className={inputClass} onClick={() => void perform(async () => { await props.onDownloadUnsynced!(); setBackupSaved(true); })}>Download unsynced encrypted copy</button>
        <label className="flex items-start gap-2"><input type="checkbox" checked={discardConfirmed} onChange={event => setDiscardConfirmed(event.target.checked)} />I saved that copy and understand this device’s unsynced changes will be discarded.</label>
        <button type="button" disabled={busy || !backupSaved || !discardConfirmed} className={inputClass} onClick={() => void perform(props.onUseSynced!)}>Use synced chats</button>
      </div>}
      <div className="mt-5 flex flex-wrap gap-x-4 gap-y-3 text-[12px] text-[var(--color-text-tertiary)]">
        {mode === 'unlock' && !code ? <><button type="button" onClick={() => { setMode('recover'); setError(''); setPassword(''); }}>Use recovery code</button><button type="button" onClick={() => { setMode('reset'); setError(''); setPassword(''); }}>Delete all chats and start fresh</button></>
          : (mode !== 'setup' || code) && <button type="button" disabled={busy} onClick={cancel}>Cancel</button>}
        <button type="button" disabled={busy} onClick={() => { cancel(); props.onSignOut(); }}>Sign out</button>
      </div>
      <p className="mt-6 flex items-center gap-1.5 text-[11px] text-[var(--color-text-muted)]"><ShieldCheck size={12} />Encryption keys stay on this device.</p>
    </section>
  </div>;
}
