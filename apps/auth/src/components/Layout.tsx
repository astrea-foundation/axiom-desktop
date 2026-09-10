import { useEffect, useState, type ReactNode } from "react";
import { ArrowUpRight, LockKeyhole, Moon, Sun } from "lucide-react";
import { Surface } from "@axiom/ui";
import wordmarkDark from "@axiom/brand/assets/axiom-ai-dark-logo.svg";
import wordmarkLight from "@axiom/brand/assets/axiom-ai-white-logo.svg";
import symbol from "@axiom/brand/assets/axiom-symbol-logo-colored.svg";
export { Notice, Spinner } from "@axiom/ui";

export function Layout({ children, wide = false }: { children: ReactNode; wide?: boolean }) {
  const [theme, setTheme] = useState(() => document.documentElement.dataset.theme === "dark" ? "dark" : "light");
  useEffect(() => { document.documentElement.dataset.theme = theme; }, [theme]);
  return <div className={`app-shell ${wide ? "wide-shell" : "entry-shell"}`}>
    <a href="#main" className="skip-link">Skip to content</a>
    <header className="site-header">
      <a className="brand" href="https://axiom.stream" aria-label="Axiom home">
        <img className="wordmark wordmark-dark" src={wordmarkDark} alt="Axiom" />
        <img className="wordmark wordmark-light" src={wordmarkLight} alt="Axiom" />
      </a>
      <span className="header-divider" aria-hidden="true" /><a className="header-label" href="/account">Account</a>
      <nav className="header-actions" aria-label="Site navigation">
        <a href="https://axiom.stream" className="home-link">Back to Axiom <ArrowUpRight size={15} /></a>
        <button type="button" className="icon-button theme-toggle" onClick={() => setTheme(theme === "light" ? "dark" : "light")} aria-label={`Use ${theme === "light" ? "dark" : "light"} theme`}>
          {theme === "light" ? <Moon size={17} /> : <Sun size={17} />}
        </button>
      </nav>
    </header>
    <main id="main" className={wide ? "page page-wide" : "page page-entry"}>
      {!wide && <aside className="brand-story" aria-label="About your Axiom account">
        <div className="story-eyebrow"><span className="signal-dot" /> A SPACE OF YOUR OWN</div>
        <h2>Your words.<br />Still yours<span className="signal-period">.</span></h2>
        <p className="story-copy">A little less noise.<br />A little more room to think.</p>
        <div className="story-seal"><img src={symbol} alt="" /><span>PRIVATE BY DESIGN.<br />YOURS BY DEFAULT.</span></div>
        <div className="story-note"><LockKeyhole size={16} /><p>Your account opens the door.<br />Your conversations stay on your device.</p></div>
      </aside>}
      <div className="page-content">{children}</div>
    </main>
    <footer className="site-footer">
      <span>© {new Date().getFullYear()} Axiom</span>
      <span className="footer-thought">A quieter kind of intelligence.</span>
      <nav aria-label="Legal"><a href="/privacy">Privacy</a><a href="/terms">Terms</a></nav>
    </footer>
  </div>;
}
export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <Surface className={`card ${className}`}>{children}</Surface>;
}
export function PageHeading({ eyebrow, title, children }: { eyebrow: string; title: string; children?: ReactNode }) {
  return <div className="page-heading"><p className="eyebrow">{eyebrow}</p><h1>{title}</h1>{children && <p className="lead-small">{children}</p>}</div>;
}
