/** @jsxRuntime automatic */
import { useEffect, useRef, useState } from 'react';
import { Check, Copy, Download, LockKeyhole, ShieldCheck } from 'lucide-react';

type Mode = 'setup' | 'unlock' | 'recover' | 'change-password' | 'reset';
export interface VaultScreenProps {
  initialMode: 'setup' | 'unlock' | 'change-password';
  onCancel?(): void;
  accountLabel: string;
  onUnlock(password: string): Promise<void>;
  onRecover(code: string): Promise<void>;
  onPrepare(password: string): Promise<string>;
  onCommit(): Promise<void>;
  onResetChallenge(): Promise<{ wait_seconds: number }>;
  onReset(): Promise<void>;
  onCancelPreparation(): void;
  onSignOut(): void;
  onReauthenticate?(): void;
  onDownloadUnsynced?(): Promise<void>;
  onUseSynced?(): Promise<void>;
}
const inputClass = 'w-full rounded-[10px] border border-[var(--color-border)] bg-[var(--wash-row)] px-3 py-2.5 text-[14px] outline-none focus:border-[var(--color-border-accent)]';
const buttonClass = 'rounded-[10px] bg-[var(--color-cherry)] px-4 py-2.5 text-[13px] font-medium text-on-accent hover:bg-[var(--color-cherry-bright)] disabled:opacity-40';

/** Secrets stay in transient form state; success, cancellation and navigation clear them. */
export function VaultScreen(props: VaultScreenProps) {
  const [mode, setMode] = useState<Mode>(props.initialMode);
  const [password, setPassword] = useState(''), [confirmation, setConfirmation] = useState('');
  const [recovery, setRecovery] = useState(''), [code, setCode] = useState('');
  const [acknowledged, setAcknowledged] = useState(false), [saved, setSaved] = useState(false);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const [remaining, setRemaining] = useState<number | null>(null), [phrase, setPhrase] = useState('');
  const [backupSaved, setBackupSaved] = useState(false), [discardConfirmed, setDiscardConfirmed] = useState(false);
  const [showPassword, setShowPassword] = useState(false), [confirmationTouched, setConfirmationTouched] = useState(false);
  const [capsLock, setCapsLock] = useState(false), [recovered, setRecovered] = useState(false);
  const [copied, setCopied] = useState(false), [downloaded, setDownloaded] = useState(false);
  const [resetAttempt, setResetAttempt] = useState(0);
  const passwordInput = useRef<HTMLInputElement>(null), recoveryInput = useRef<HTMLTextAreaElement>(null);
  const operation = useRef(false);
  const lifetime = useRef(0);
  useEffect(() => () => { lifetime.current++; }, []);
  const perform = async (action: () => Promise<void>, clearError = true) => {
    if (operation.current) return;
    operation.current = true;
    const scope = lifetime.current;
    if (clearError) setError(''); setBusy(true);
    try { await action(); }
    catch (failure) { if (scope === lifetime.current) setError(failure instanceof Error ? failure.message : 'Couldn’t continue. Try again.'); }
    finally { if (scope === lifetime.current) { operation.current = false; setBusy(false); } }
  };
  useEffect(() => {
    if (!busy && !code) (mode === 'recover' ? recoveryInput : passwordInput).current?.focus();
  }, [mode, code, busy]);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 3000);
    return () => clearTimeout(timer);
  }, [copied]);
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
    }).catch(failure => { if (!cancelled) {
      const message = failure instanceof Error ? failure.message : '';
      setError(/sign in again|session expired/i.test(message) ? message : 'Couldn’t prepare deletion. Try again.');
    } });
    return () => { cancelled = true; if (timer) clearInterval(timer); };
  }, [mode, resetAttempt]);
  const cancel = () => {
    lifetime.current++; props.onCancelPreparation();
    setPassword(''); setConfirmation(''); setRecovery(''); setCode(''); setPhrase('');
    operation.current = false;
    setAcknowledged(false); setSaved(false); setBackupSaved(false); setDiscardConfirmed(false); setRemaining(null); setBusy(false); setError(''); setMode(props.initialMode);
    setShowPassword(false); setConfirmationTouched(false); setCapsLock(false); setRecovered(false); setCopied(false); setDownloaded(false);
    if (props.initialMode === 'change-password') props.onCancel?.();
  };
  const newPassword = mode === 'setup' || mode === 'change-password';
  const title = code ? 'Save your recovery code' : mode === 'setup' ? 'Create an encryption password'
    : mode === 'unlock' ? 'Unlock your chats' : mode === 'recover' ? 'Use your recovery code'
    : mode === 'change-password' ? 'Choose a new encryption password' : 'Delete all web chats?';
  const description = mode === 'setup' ? 'This password protects your saved chats and unlocks them on other devices. It’s separate from your account sign-in.'
    : mode === 'unlock' ? 'Enter the encryption password you chose for web chat. Signing in alone doesn’t unlock your saved chats.'
    : mode === 'recover' ? 'Paste your code or open the file you saved. You’ll then choose a new password and recovery code. Your chats will be kept.'
    : mode === 'change-password' ? recovered ? 'Your recovery code worked. Choose a new password to protect your chats.' : 'Your chats will be kept. This replaces your encryption password and recovery code.' : '';
  const mismatch = !!confirmation && password !== confirmation && (confirmationTouched || confirmation.length >= password.length);
  const reauthenticationRequired = /sign in again|session expired/i.test(error);
  const download = () => {
    const url = URL.createObjectURL(new Blob([code + '\n'], { type: 'text/plain;charset=utf-8' }));
    const link = document.createElement('a'); link.href = url; link.download = 'axiom-recovery-code.txt'; link.click();
    setDownloaded(true);
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return <div className="flex h-full min-h-0 flex-col overflow-y-auto px-5 py-8">
    <section className="glass-panel-solid shadow-glass-card m-auto w-full max-w-[440px] shrink-0 rounded-[22px] border border-[var(--color-border)] p-6 sm:p-8" aria-labelledby="vault-title" aria-busy={busy}>
      <LockKeyhole size={24} className="mb-5 text-[var(--color-cherry)]" aria-hidden="true" />
      {newPassword && <p className="mb-2 text-[11px] text-[var(--color-text-tertiary)]">Step {code ? '2' : '1'} of 2 · {code ? 'Recovery code' : 'Password'}</p>}
      <h1 id="vault-title" className="text-[23px] font-medium tracking-tight">{title}</h1>
      <p className="mt-1 text-[12px] text-[var(--color-text-tertiary)]">Signed in as {props.accountLabel}</p>
      {!code && description && <p className="mt-4 text-[13px] leading-5 text-[var(--color-text-secondary)]">{description}</p>}
      {code ? <div className="mt-5 space-y-4">
        <p className="text-[13px] leading-5 text-[var(--color-text-secondary)]">Save this in a password manager or another safe place. Treat it like a password: it can unlock your chats if you forget yours.</p>
        {mode !== 'setup' && <p className="text-[12px] leading-5 text-[var(--color-text-secondary)]">This replaces your previous recovery code when you save the new password.</p>}
        <div className="flex flex-wrap gap-2">
          <button type="button" autoFocus disabled={busy} className={buttonClass + ' flex items-center gap-2'} onClick={download}><Download size={14} />Download recovery code</button>
          <button type="button" aria-label="Copy recovery code" disabled={busy} className={inputClass + ' !w-auto flex items-center gap-2'} onClick={() => void perform(async () => {
            try { await navigator.clipboard.writeText(code); setCopied(true); }
            catch { throw new Error('Couldn’t copy the code. Download it or open “Show recovery code” to copy it yourself.'); }
          })}>{copied ? <Check size={14} /> : <Copy size={14} />}<span role="status">{copied ? 'Copied' : 'Copy'}</span></button>
        </div>
        {downloaded && <p role="status" className="text-[12px] text-[var(--color-text-secondary)]">Download started. Keep the file somewhere safe.</p>}
        <details className="text-[12px] text-[var(--color-text-secondary)]"><summary className="cursor-pointer">Show recovery code</summary><textarea aria-label="Your recovery code" readOnly spellCheck={false} value={code} onFocus={event => event.currentTarget.select()} className={inputClass + ' mt-2 min-h-24 resize-y font-mono !text-[12px]'} /></details>
        <label className="flex items-start gap-2 text-[13px]"><input type="checkbox" disabled={busy} checked={saved} onChange={event => setSaved(event.target.checked)} className="mt-1" />I saved my recovery code.</label>
        <button type="button" disabled={!saved || busy} className={buttonClass + ' w-full'} onClick={() => void perform(props.onCommit)}>{busy ? 'Saving…' : mode === 'setup' ? 'Open chats' : 'Save new password'}</button>
      </div> : mode === 'reset' ? <div className="mt-5 space-y-4">
        <p className="text-[13px] leading-6 text-[var(--color-text-secondary)]">This permanently deletes all your saved web chats and files on every device. Axiom cannot restore them. Your account and balance will stay.</p>
        <p className="text-[12px] leading-5 text-[var(--color-text-secondary)]">Take a moment to read this. You can confirm after the 20-second countdown.</p>
        <label className="block text-[12px]">Type DELETE ALL CHATS<input aria-label="Confirm deletion" disabled={busy} autoComplete="off" value={phrase} onChange={event => setPhrase(event.target.value)} onKeyDown={event => { if (event.key === 'Enter') event.preventDefault(); }} className={inputClass + ' mt-2'} /></label>
        <label className="flex items-start gap-2 text-[13px]"><input type="checkbox" disabled={busy} checked={acknowledged} onChange={event => setAcknowledged(event.target.checked)} className="mt-1" />I understand this deletes my web chats and files.</label>
        <button type="button" disabled={remaining !== 0 || phrase !== 'DELETE ALL CHATS' || !acknowledged || busy} className={buttonClass + ' w-full !bg-[var(--color-danger-strong)] !text-white'} onClick={() => void perform(props.onReset)}>{busy ? 'Deleting…' : remaining === null ? 'Preparing confirmation…' : remaining > 0 ? `Delete all chats (${remaining}s)` : 'Delete all chats and start fresh'}</button>
      </div> : <form className="mt-5 space-y-4" onSubmit={event => {
        event.preventDefault();
        if (busy) return;
        if (newPassword && (password !== confirmation || password.length < 12 || !acknowledged)) return;
        const input = mode === 'recover' ? recovery : password;
        void perform(async () => {
          if (mode === 'unlock') await props.onUnlock(input);
          else if (mode === 'recover') { const scope = lifetime.current; await props.onRecover(input); if (scope === lifetime.current) { setMode('change-password'); setRecovered(true); setAcknowledged(false); } }
          else { const scope = lifetime.current, next = await props.onPrepare(input); if (scope === lifetime.current) setCode(next); else props.onCancelPreparation(); }
          setPassword(''); setConfirmation(''); setRecovery(''); setShowPassword(false); setCapsLock(false); setConfirmationTouched(false);
        });
      }}>
        {mode === 'recover' ? <>
          <label className="block text-[12px]">Recovery code<textarea ref={recoveryInput} disabled={busy} aria-label="Recovery code" autoComplete="off" spellCheck={false} value={recovery} onChange={event => { setRecovery(event.target.value); setError(''); }} className={inputClass + ' mt-2 min-h-24 resize-y font-mono text-[12px]'} maxLength={256} required /></label>
          <label className="inline-block cursor-pointer text-[12px] underline">Open recovery file<input type="file" disabled={busy} accept="text/plain,.txt" className="hidden" onChange={event => {
            const file = event.target.files?.[0]; event.target.value = '';
            if (!file) return;
            if (file.size > 1024) { setError('Invalid recovery file'); return; }
            const scope = lifetime.current; void file.text().then(value => { if (scope === lifetime.current) setRecovery(value.trim()); }).catch(() => { if (scope === lifetime.current) setError('Couldn’t read this recovery file'); });
          }} /></label>
        </> : <>
          <label className="block text-[12px]">Encryption password<input ref={passwordInput} type={showPassword ? 'text' : 'password'} disabled={busy} autoComplete={newPassword ? 'section-chat-encryption new-password' : 'section-chat-encryption current-password'} aria-label="Encryption password" aria-describedby={newPassword ? 'vault-password-help' : undefined} value={password} onChange={event => { setPassword(event.target.value); setError(''); }} onKeyUp={event => setCapsLock(event.getModifierState('CapsLock'))} onBlur={() => setCapsLock(false)} minLength={newPassword ? 12 : undefined} maxLength={1024} className={inputClass + ' mt-2'} required /></label>
          {newPassword && <>
            <p id="vault-password-help" className="!mt-2 text-[12px] leading-5 text-[var(--color-text-secondary)]">At least 12 characters. A few words work well; numbers and symbols are optional.</p>
            <label className="block text-[12px]">Confirm password<input type={showPassword ? 'text' : 'password'} disabled={busy} autoComplete="section-chat-encryption new-password" aria-label="Confirm encryption password" aria-invalid={mismatch || undefined} aria-describedby={confirmation ? 'vault-password-match' : undefined} value={confirmation} onChange={event => setConfirmation(event.target.value)} onKeyUp={event => setCapsLock(event.getModifierState('CapsLock'))} onBlur={() => { setConfirmationTouched(true); setCapsLock(false); }} maxLength={1024} className={inputClass + ' mt-2'} required /></label>
            {confirmation && <p id="vault-password-match" role="status" className={'!mt-2 text-[12px] ' + (mismatch ? 'text-[var(--color-danger-strong)]' : 'text-[var(--color-text-secondary)]')}>{mismatch ? 'Passwords don’t match.' : password === confirmation ? 'Passwords match.' : 'Enter the same password again.'}</p>}
          </>}
          <label className="flex items-center gap-2 text-[12px] text-[var(--color-text-secondary)]"><input type="checkbox" disabled={busy} checked={showPassword} onChange={event => setShowPassword(event.target.checked)} />{newPassword ? 'Show passwords' : 'Show password'}</label>
          {capsLock && <p role="status" className="!mt-2 text-[12px] text-[var(--color-text-secondary)]">Caps Lock is on.</p>}
        </>}
        {newPassword && <>
          <p className="text-[12px] leading-5 text-[var(--color-text-secondary)]">Axiom cannot recover your encryption password. Save your recovery code. Without either, your chats cannot be decrypted.</p>
          <label className="flex items-start gap-2 text-[12px]"><input type="checkbox" checked={acknowledged} onChange={event => setAcknowledged(event.target.checked)} className="mt-0.5" />I understand and will save my recovery code.</label>
        </>}
        <button type="submit" disabled={busy || (mode === 'recover' ? !recovery.trim() : !password) || (newPassword && (password.length < 12 || password !== confirmation || !acknowledged))} className={buttonClass + ' w-full'}>{busy ? mode === 'unlock' ? 'Unlocking chats…' : mode === 'recover' ? 'Checking recovery code…' : 'Creating recovery code…' : mode === 'unlock' ? 'Unlock' : mode === 'recover' ? 'Recover chats' : 'Continue'}</button>
      </form>}
      {error && <p role="alert" className="mt-4 text-[12px] text-[var(--color-danger-strong)]">{error} {reauthenticationRequired && props.onReauthenticate && <button type="button" disabled={busy} onClick={props.onReauthenticate} className="underline">Sign in again</button>}</p>}
      {mode === 'reset' && error && remaining === null && !reauthenticationRequired && <button type="button" className={inputClass + ' mt-3'} onClick={() => { setError(''); setResetAttempt(value => value + 1); }}>Try again</button>}
      {error.includes('encrypted local copy was kept') && props.onDownloadUnsynced && props.onUseSynced && <div className="mt-4 space-y-3 text-[12px]">
        <button type="button" disabled={busy} className={inputClass} onClick={() => void perform(async () => { await props.onDownloadUnsynced!(); setBackupSaved(true); }, false)}>Download unsynced encrypted copy</button>
        <label className="flex items-start gap-2"><input type="checkbox" checked={discardConfirmed} onChange={event => setDiscardConfirmed(event.target.checked)} />I saved that copy and understand this device’s unsynced changes will be discarded.</label>
        <button type="button" disabled={busy || !backupSaved || !discardConfirmed} className={inputClass} onClick={() => void perform(props.onUseSynced!, false)}>Use synced chats</button>
      </div>}
      <div className="mt-5 flex flex-wrap gap-x-4 gap-y-3 text-[12px] text-[var(--color-text-tertiary)]">
        {mode === 'unlock' && !code ? <><button type="button" disabled={busy} onClick={() => { setMode('recover'); setError(''); setPassword(''); setShowPassword(false); setCapsLock(false); }}>Use recovery code</button><button type="button" disabled={busy} onClick={() => { setMode('reset'); setError(''); setPassword(''); setShowPassword(false); setCapsLock(false); }}>Delete all chats and start fresh</button></>
          : (mode !== 'setup' || code) && <button type="button" disabled={busy} onClick={cancel}>Cancel</button>}
        <button type="button" disabled={busy} onClick={() => { cancel(); props.onSignOut(); }}>Sign out</button>
      </div>
      <p className="mt-6 flex items-center gap-1.5 text-[11px] text-[var(--color-text-muted)]"><ShieldCheck size={12} />Your encryption password never leaves this browser.</p>
    </section>
  </div>;
}
