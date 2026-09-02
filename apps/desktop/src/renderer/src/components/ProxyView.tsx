import { ArrowLeft, Check, ChevronDown, Copy, Power, RotateCcw, ShieldCheck } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";

const LOCAL_TOKEN = "axiom-local";
const DEFAULT_PORT = 8484;

const ERRORS = [
  { id: "1", at: "12s", model: "deepseek-v4-flash", detail: "timeout" },
  { id: "2", at: "8m", model: "kimi-k2.5", detail: "401" },
];

const well = "shadow-glass-card relative rounded-2xl bg-[var(--surface-card)]";

type Copied = "url" | "token" | null;

export function ProxyView({
  onInspecting,
}: {
  onInspecting?: (inspecting: boolean) => void;
}) {
  const [running, setRunning] = useState(true);
  const [appliedPort, setAppliedPort] = useState(DEFAULT_PORT);
  const [portDraft, setPortDraft] = useState(String(DEFAULT_PORT));
  const [copied, setCopied] = useState<Copied>(null);
  const [errorsOpen, setErrorsOpen] = useState(false);
  const [inspecting, setInspecting] = useState(false);
  const [restarting, setRestarting] = useState(false);

  const port = Number.parseInt(portDraft, 10);
  const portValid = Number.isInteger(port) && port >= 1 && port <= 65535;
  const dirty = portValid && port !== appliedPort;
  const liveUrl = `http://127.0.0.1:${appliedPort}/v1`;

  useEffect(() => {
    onInspecting?.(inspecting);
    return () => onInspecting?.(false);
  }, [inspecting, onInspecting]);

  const copy = async (which: Exclude<Copied, null>, value: string) => {
    await navigator.clipboard.writeText(value);
    setCopied(which);
    window.setTimeout(() => setCopied(null), 1200);
  };

  const restart = () => {
    if (!portValid) return;
    setAppliedPort(port);
    if (!running && !dirty) return;
    setRestarting(true);
    setRunning(false);
    window.setTimeout(() => {
      setRunning(true);
      setRestarting(false);
    }, 280);
  };

  if (inspecting) {
    return (
      <div className="relative flex h-full min-h-0 flex-col">
        <div className="mx-auto flex w-full max-w-[760px] flex-1 flex-col px-6 pb-8 pt-20">
          <button
            type="button"
            onClick={() => setInspecting(false)}
            className="mb-8 flex items-center gap-2 self-start text-[13px] text-[var(--color-text-tertiary)] transition-colors hover:text-[var(--color-text-primary)]"
          >
            <ArrowLeft size={14} />
            Back
          </button>
          <div className={`${well} px-5 py-8`}>
            <div className="flex h-12 w-12 items-center justify-center rounded-full bg-[var(--color-cherry-glow)] text-[var(--color-cherry)]">
              <ShieldCheck size={22} />
            </div>
            <h1 className="mt-4 text-[22px] font-medium tracking-tight">Attestation</h1>
            <p className="mt-2 text-[13.5px] text-[var(--color-text-tertiary)]">
              Evidence will land here.
            </p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="title-fade relative flex h-full min-h-0 flex-col overflow-y-auto">
      <div className="mx-auto flex w-full max-w-[760px] flex-1 flex-col gap-3 px-6 pb-8 pt-[88px]">
        <div className={`${well} overflow-hidden px-5 pb-5 pt-5`}>
          <div className="relative flex items-start justify-between gap-3">
            <div className="flex min-w-0 items-center gap-3">
              <div className="relative flex h-11 w-11 shrink-0 items-center justify-center">
                <div
                  className={[
                    "relative flex h-11 w-11 items-center justify-center rounded-full",
                    running
                      ? "bg-[var(--color-cherry)] text-on-accent"
                      : "bg-[var(--color-cherry-glow)] text-[var(--color-text-tertiary)]",
                  ].join(" ")}
                >
                  <ShieldCheck size={20} />
                </div>
              </div>
              <div className="min-w-0">
                <div className="flex items-center gap-2">
                  <span
                    className={[
                      "h-1.5 w-1.5 rounded-full",
                      running ? "bg-[var(--color-success)]" : "bg-[var(--color-text-muted)]",
                    ].join(" ")}
                  />
                  <span className="text-[16px] font-medium tracking-tight">
                    {restarting ? "Restarting" : running ? "Running" : "Stopped"}
                  </span>
                </div>
                <p className="mt-0.5 text-[12.5px] text-[var(--color-text-tertiary)]">
                  OpenAI loopback for local clients
                </p>
              </div>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <IconButton
                title={running ? "Stop" : "Start"}
                onClick={() => setRunning((value) => !value)}
                accent={!running}
              >
                <Power size={15} />
              </IconButton>
              <IconButton
                title={dirty ? "Restart to apply" : "Restart"}
                onClick={restart}
                disabled={!dirty && !running}
                accent={dirty}
              >
                <RotateCcw size={15} className={restarting ? "animate-spin" : undefined} />
              </IconButton>
            </div>
          </div>

          <button
            type="button"
            onClick={() => void copy("url", liveUrl)}
            className="relative mt-5 flex w-full items-center gap-2 rounded-[8px] border border-[var(--color-border)] bg-[var(--color-bg-input)] px-3.5 py-2.5 text-left transition-colors hover:border-[var(--color-border-accent)]"
            title="Copy URL"
          >
            <span className="min-w-0 flex-1 truncate font-mono text-[13px] text-[var(--color-text-secondary)]">
              {liveUrl}
            </span>
            {copied === "url" ? (
              <Check size={14} className="shrink-0 text-[var(--color-cherry)]" />
            ) : (
              <Copy size={14} className="shrink-0 text-[var(--color-text-tertiary)]" />
            )}
          </button>
        </div>

        <div className={`${well} px-5 py-1.5`}>
          <Field label="Port">
            <input
              type="text"
              inputMode="numeric"
              value={portDraft}
              onChange={(event) =>
                setPortDraft(event.target.value.replace(/[^\d]/g, "").slice(0, 5))
              }
              className="w-full bg-transparent font-mono text-[13px] text-[var(--color-text-primary)] outline-none"
              aria-label="Port"
            />
          </Field>
          <Field
            label="Token"
            action={
              <IconButton
                title={copied === "token" ? "Copied" : "Copy token"}
                onClick={() => void copy("token", LOCAL_TOKEN)}
              >
                {copied === "token" ? (
                  <Check size={14} className="text-[var(--color-cherry)]" />
                ) : (
                  <Copy size={14} />
                )}
              </IconButton>
            }
          >
            <span className="font-mono text-[13px] text-[var(--color-text-secondary)]">••••••••</span>
          </Field>
          {dirty ? (
            <p className="px-0.5 pb-2.5 text-[12px] text-[var(--color-cherry)]">Restart to apply</p>
          ) : null}
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div className={`${well} px-4 py-3.5`}>
            <div className="text-[11px] uppercase tracking-[0.08em] text-[var(--color-text-tertiary)]">
              Tokens
            </div>
            <div className="mt-1 font-mono text-[18px] tracking-tight text-[var(--color-text-primary)]">
              12.4k
            </div>
          </div>
          <button
            type="button"
            onClick={() => setErrorsOpen((open) => !open)}
            className={`${well} px-4 py-3.5 text-left transition-colors hover:bg-[var(--surface-card-hover)]`}
          >
            <div className="flex items-center justify-between gap-2">
              <span className="text-[11px] uppercase tracking-[0.08em] text-[var(--color-text-tertiary)]">
                Errors
              </span>
              <ChevronDown
                size={14}
                className={[
                  "text-[var(--color-text-tertiary)] transition-transform",
                  errorsOpen ? "rotate-180" : "",
                ].join(" ")}
              />
            </div>
            <div className="mt-1 font-mono text-[18px] tracking-tight text-[var(--color-text-primary)]">
              {ERRORS.length}
            </div>
          </button>
        </div>

        {errorsOpen ? (
          <div className={`${well} px-4 py-2`}>
            {ERRORS.map((entry) => (
              <div
                key={entry.id}
                className="flex items-baseline gap-3 border-b border-[var(--color-border)] py-2.5 last:border-b-0 font-mono text-[12.5px]"
              >
                <span className="w-8 shrink-0 text-[var(--color-text-muted)]">{entry.at}</span>
                <span className="min-w-0 flex-1 truncate text-[var(--color-text-secondary)]">
                  {entry.model}
                </span>
                <span className="text-[var(--color-danger)]">{entry.detail}</span>
              </div>
            ))}
          </div>
        ) : null}

        <button
          type="button"
          onClick={() => setInspecting(true)}
          className={`${well} flex items-center gap-3 px-4 py-3.5 text-left transition-colors hover:bg-[var(--surface-card-hover)]`}
        >
          <span className="flex h-8 w-8 items-center justify-center rounded-full bg-[var(--color-cherry-glow)] text-[var(--color-cherry)]">
            <ShieldCheck size={15} />
          </span>
          <span className="min-w-0 flex-1">
            <span className="block text-[13.5px] text-[var(--color-text-primary)]">Attestation</span>
            <span className="block text-[12px] text-[var(--color-text-tertiary)]">Inspect TEE evidence</span>
          </span>
        </button>
      </div>
    </div>
  );
}

function Field({
  label,
  children,
  action,
}: {
  label: string;
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="flex min-h-11 items-center gap-4 border-b border-[var(--color-border)] last:border-b-0">
      <span className="w-12 shrink-0 text-[12.5px] text-[var(--color-text-tertiary)]">{label}</span>
      <div className="min-w-0 flex-1">{children}</div>
      {action ? <div className="shrink-0">{action}</div> : null}
    </div>
  );
}

function IconButton({
  title,
  onClick,
  disabled,
  accent,
  children,
}: {
  title: string;
  onClick: () => void;
  disabled?: boolean;
  accent?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      onClick={onClick}
      disabled={disabled}
      className={[
        "flex h-9 w-9 items-center justify-center rounded-full transition-colors disabled:cursor-not-allowed disabled:opacity-35",
        accent
          ? "bg-[var(--color-cherry)] text-on-accent hover:bg-[var(--color-cherry-bright)]"
          : "text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-surface-hover)] hover:text-[var(--color-text-primary)]",
      ].join(" ")}
    >
      {children}
    </button>
  );
}
