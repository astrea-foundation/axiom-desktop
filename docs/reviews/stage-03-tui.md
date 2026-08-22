# Stage 03 Review — Terminal UI

Decision: **PASS (early-development scope)**

- Ratatui owns the current terminal viewport; this is not OS fullscreen.
- Alternate-screen/raw-mode/mouse guards restore on every normal/error exit.
- TestBackend covers 42×10 and 100×30, long paths, scrolling, Unicode theme
  content, and control-character stripping.
- A real 100×30 PTY run rendered the header and `UNATTESTED DEV`, accepted Esc,
  exited zero, and emitted both alternate-screen enter and leave sequences.
- Rendering consumes semantic events and contains no agent/provider behavior.

Deferred polish: no-color/ASCII palettes, transcript search, selection, and
interactive expansion are tracked as pre-1.0 UI work rather than hidden claims.
