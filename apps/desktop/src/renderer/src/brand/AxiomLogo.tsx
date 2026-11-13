import symbolUrl from "@axiom/brand/assets/axiom-symbol-logo-colored.svg";

interface LogoProps {
  className?: string;
}

export function AxiomMark({ className = "" }: LogoProps) {
  return <img src={symbolUrl} alt="" draggable={false} className={className} />;
}
