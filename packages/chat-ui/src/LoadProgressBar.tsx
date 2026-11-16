/** @jsxRuntime automatic */

/**
 * Accessible loading bar. `value` (0–1) is shown only when it reflects observed
 * work; omit it for an indeterminate bar. `active` marks the span of the step
 * currently running after the completed fill, without claiming a percentage.
 */
export function LoadProgressBar({ label, value = null, active = 0, valueText, start, end, className = "" }: {
  label: string;
  value?: number | null;
  active?: number;
  valueText?: string;
  start?: string;
  end?: string;
  className?: string;
}) {
  const known = value !== null && Number.isFinite(value) ? Math.min(Math.max(value, 0), 1) : null;
  const span = known === null ? 0 : Math.min(Math.max(active, 0), 1 - known);
  return <div className={`load-progress ${className}`} role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={100}
    aria-valuenow={known === null ? undefined : Math.round(known * 100)} aria-valuetext={valueText}>
    <div className="load-progress-track">
      {known === null ? <span className="load-progress-sweep" /> : <>
        <span className="load-progress-fill" style={{ width: `${known * 100}%` }} />
        {span > 0 ? <span className="load-progress-active" style={{ left: `${known * 100}%`, width: `${span * 100}%` }} /> : null}
      </>}
    </div>
    {start || end ? <div className="load-progress-meta" aria-hidden="true"><span>{start}</span><span>{end}</span></div> : null}
  </div>;
}
