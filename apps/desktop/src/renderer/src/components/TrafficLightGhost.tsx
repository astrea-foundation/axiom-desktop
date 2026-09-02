import { isMac, useFullScreen, useWindowActive } from "../lib/window-chrome";

// Electron leaves the macOS traffic lights fully transparent while the window
// is inactive instead of drawing them grey (electron#44034). These replicas
// take their place on blur; the real buttons sit above web content, so the
// moment AppKit draws them (focus, hover) they cover the ghosts exactly.
const DOT_CENTERS = [20, 40, 60];

export function TrafficLightGhost() {
  const active = useWindowActive();
  const fullScreen = useFullScreen();

  if (!isMac || active || fullScreen) return null;

  return (
    <div className="pointer-events-none fixed left-0 top-0 z-50" aria-hidden="true">
      {DOT_CENTERS.map((center) => (
        <span
          key={center}
          className="absolute h-3 w-3 rounded-full"
          style={{
            left: center - 6,
            top: 12,
            background: "var(--chrome-ghost-dot)",
            boxShadow: "inset 0 0 0 0.5px var(--chrome-ghost-ring)",
          }}
        />
      ))}
    </div>
  );
}
