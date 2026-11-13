import { X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import lockup from "../assets/astrea/astrea-lockup.svg";
import field from "../assets/astrea/field-banner.webm";
import poster from "../assets/astrea/field-banner-poster.png";
import "./AstreaBanner.css";

const EXIT_MS = 200;

export function AstreaBanner() {
  const [ready, setReady] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [closing, setClosing] = useState(false);
  const claim = useRef<Promise<boolean> | null>(null);
  const [reducedMotion, setReducedMotion] = useState(() => window.matchMedia("(prefers-reduced-motion: reduce)").matches);

  useEffect(() => {
    let active = true;
    const api = window.axiomDesktop?.astreaBanner;
    if (!api) return;
    // Reuse the claim across StrictMode's effect replay. A renderer reload or
    // later launch gets a fresh claim and cannot repeat the introduction.
    claim.current ??= api.claim();
    void claim.current.then((show) => {
      if (active) { setDismissed(!show); setReady(true); }
    }).catch(() => { /* Keep the banner hidden if desktop state is unavailable. */ });
    return () => { active = false; };
  }, []);

  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReducedMotion(media.matches);
    media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);

  useEffect(() => {
    if (!closing) return;
    const finish = window.setTimeout(() => setDismissed(true), reducedMotion ? 0 : EXIT_MS);
    return () => window.clearTimeout(finish);
  }, [closing, reducedMotion]);

  if (!ready || dismissed) return null;

  const close = () => {
    setClosing(true);
  };

  return (
    <aside
      className="astrea-banner"
      aria-label="Runs on Astrea"
      data-closing={closing}
      inert={closing}
      aria-hidden={closing}
    >
      <div className="astrea-banner__motion" aria-hidden="true">
        <img className="astrea-banner__field" src={poster} alt="" />
        {!reducedMotion ? (
          <video className="astrea-banner__field" src={field} poster={poster}
            autoPlay loop muted playsInline preload="auto" />
        ) : null}
      </div>
      <a className="astrea-banner__link" href="https://astrea.systems" target="_blank" rel="noopener noreferrer" aria-label="Astrea — opens astrea.systems in your browser">
        <span className="astrea-banner__copy">RUNS ON</span>
        <img className="astrea-banner__lockup" src={lockup} alt="Astrea" width="60" height="18" draggable={false} />
      </a>
      <button className="astrea-banner__close" type="button" onClick={close} aria-label="Dismiss Astrea message" title="Dismiss">
        <X size={12} strokeWidth={1.25} aria-hidden="true" />
      </button>
    </aside>
  );
}
