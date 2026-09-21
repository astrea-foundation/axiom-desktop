# Desktop redesign — hand-off notes (branch `redesign`)

> Historical record, archived 2026-09-21. This preserves the original proposal
> or dated evidence, including superseded behavior and unfinished checklists.
> It is not a current implementation or deployment runbook. See the
> [current guides](../README.md) and [validation follow-up](../validation.md).

Written for the developer merging this branch and for any coding agent working
in `apps/desktop` afterwards. It explains what changed, why, what it leads to,
and the rules the new code expects you to keep. The original redesign was
presentation-only; the current tree also includes the later native-account,
account-scoped state, billing-extension, and cross-platform packaging work
called out explicitly below.

Redesign base: `origin/main` at the time of branching. Original redesign scope:
`apps/desktop/` only.

---

## 1. What this branch is

A full reskin of the Electron renderer onto the Axiom webapp redesign
(light "frosted glass" language), plus a dark theme, native-feeling window
chrome per platform, a screenshot harness that covers every state in both
themes, and the brand/packaging assets needed to ship a branded build.

Headline outcomes:

- Light and dark themes on one semantic token set (`--surface-*`, `--wash-*`,
  `--ring-*` …), switched by `data-theme` on `<html>`. Components never
  hardcode white or ink any more.
- macOS keeps native traffic lights; when the sidebar is collapsed the stage
  card carves a tab for them (`clip-path: shape()`). Windows/Linux are
  frameless with app-drawn controls.
- Sidebar collapses with a 300 ms slide and stays mounted; thread rows show
  idle / unread / working dots.
- Composer, bubbles, disclosures, model picker, TEE badge, proxy page and
  settings all restyled. Assistant replies have no bubble; user messages do.
- `pnpm capture` now produces 24 PNGs (12 states × light/dark).
- Brand assets are generated from designer masters (`pnpm brand:assets`),
  `electron-builder.yml` exists, macOS gets an About panel and menu.

## 2. Run and verify

```text
pnpm install
pnpm desktop                     # builds axiomcli, starts electron-vite dev
pnpm --filter axiom-desktop test # 24 node:test cases
pnpm --filter axiom-desktop typecheck
pnpm --filter axiom-desktop capture   # ~/axiom-desktop-captures/{light,dark}-*.png
```

- `AXIOMCLI_SIDECAR=/path/to/binary-or-script` overrides the sidecar in dev
  **and** in capture mode (packaged builds ignore it, as before).
- `AXIOM_DESKTOP_PLATFORM_PREVIEW=win32 pnpm desktop` on a Mac renders the
  Windows/Linux controls and right-side cutout while hiding the macOS traffic
  lights. The BrowserWindow itself is still macOS, but the visible chrome is
  renderer-owned in this mode.
- `electron-vite dev` hot-reloads the renderer only. Anything under
  `src/main` or `src/preload` needs the app restarted.
- Capture shots: `conversation, welcome, models, signin, proxy, folder, markdown,
  markdown-top, markdown-open, errors, settings, collapsed`. `collapsed` is
  taken windowed + inactive so the notch and the grey ghost traffic lights
  are visible. Shots are driven by `window.__axiomDemo` hooks (typed as
  `DemoApi` in `src/renderer/src/types.ts`); a new UI state needs a hook and
  a `shots` entry in `src/main/index.ts`.
- Design review on Dmitry's machine used a mock ACP sidecar (fixtures for
  threads, folders, models, TEE states, streaming, tool calls, permissions).
  It is deliberately **not** in this branch; ask if you want it.

## 3. What changed, by area

### 3.1 Tokens and CSS (`src/renderer/src/index.css`)

- Fonts: Geist → **DM Sans** (`--font-sans`), **DM Mono** (`--font-mono`),
  and local **PP Cirka** (`--font-display`, weights 300/400/700, used only
  for the welcome headline). Font files live in `src/renderer/src/brand/fonts/`.
- `@theme` palette is now light-first. The `--color-cherry*` names were kept
  to avoid churn but now mean **ink** (`#101014` light / `#f4f4f6` dark):
  it is the primary-action fill (Send/Stop, Continue, avatar, running shield).
  Brand orange is `--color-brand` (`#ff7653`, identical in both themes) and is
  used only for the unread/working thread dot and the selected-model check.
  New: `--color-on-accent` (white → ink in dark; put it on anything drawn on a
  cherry fill), `--color-warning*`, `--color-danger*`, `--color-success`.
- Radius scale tightened (xs 4 / sm 6 / md 8 / lg 10 / xl 12 / 2xl 14 / 3xl 16).
  Marquee surfaces use explicit radii instead: stage card 22 px, composer
  22 px, bubbles 19 px, popovers 10 px, inputs 8 px, sidebar rows 9 px, menu
  items 6 px.
- A semantic layer on `:root` (about 50 variables) with a complete override
  under `:root[data-theme="dark"]`:
  - surfaces `--surface-stage|card|card-hover|tile|float|pill|pill-hover|
    bubble-user|button-secondary(-hover)|glass(-hover|-focus)|track`
  - washes `--wash-chip|chip-hover|hover|hover-strong|row|code|table-head`
  - lines `--ring-hairline(-hover)|focus-halo|focus-visible`, `--border-hover`,
    `--border-quote`, `--link-underline(-hover)`, `--scrollbar-thumb(-hover)`
  - depth `--shade-xs|soft|button`, `--glass-inset(-strong|-soft)`, `--stage-ring`
  - misc `--dot-idle`, `--blob-fill`, `--chrome-ghost-dot|ring`
  Light values are white-alpha over a bright canvas; dark values are
  white-alpha over a deep one, so the same class carries both themes.
- CSS that cannot be a token (gradients, multi-layer shadows, filters) is
  written as a light rule followed by a `:root[data-theme="dark"] .x` twin:
  `.app-canvas`, `.stage-shadow`, `.glass-panel(-solid)`, `.shadow-glass-*`,
  `.cherry-glow`, `.brand-mono`. There is no `dark:` variant and no
  `prefers-color-scheme` rule; the token swap *is* the theme.
- New blocks: `.app-canvas` (renamed from `.light-canvas`), the stage-frame
  notch (`@property --nt`, `.stage-frame`, `.stage-shadow`, `.stage-ring`,
  `.stage-card`), `.composer-shell`, `.title-fade`, `.bg-blob`, `.thread-dot*`,
  `.ax-pill-button`, `.brand-mono`, `.unmask*`, GFM table prose rules.
- Removed: `.pad-titlebar-overlay` (Windows title-bar overlay inset),
  `lib/sakura.ts` + `lib/sakura.css`, the painted header scrim.

### 3.2 Theme plumbing

- `src/renderer/src/lib/theme.ts` (new) owns the preference
  (`localStorage["axiom.theme"]`: light | dark | system, default system),
  resolves `system` via `prefers-color-scheme`, stamps
  `document.documentElement.dataset.theme` at module load (before first
  render), and mirrors it to main with `window.axiomDesktop.setTheme(preference,
  resolved)`. `useTheme()` for reads, `setThemePreference()` for writes;
  `{ persist: false }` exists for transient switches (capture hook).
- Preload: `DesktopApi.setTheme` → IPC `window:set-theme`.
- Main: sets `nativeTheme.themeSource` (so OS-drawn menus/dialogs follow) and
  `window.setBackgroundColor("#f2f2f4" | "#141416")`; the window is created
  with the OS preference so there is no flash. Keep `BACKGROUND` /
  `BACKGROUND_DARK` in `src/main/index.ts` equal to the renderer's base surface.
- Settings → Appearance has a real Light / Dark / System radiogroup (roving
  tabindex, arrow keys).

### 3.3 Window chrome per platform

- `src/main/window-chrome.ts` (new, unit-tested): darwin uses
  `{ titleBarStyle: "hiddenInset" }`; Windows and Linux use `{ frame: false }`
  with app-drawn controls in the outer canvas. Never add a custom
  `trafficLightPosition` on macOS: it re-parents the buttons and they render
  empty when the window is inactive (electron#27295).
- Electron also draws the traffic lights fully transparent while the window
  is blurred (electron#44034). `TrafficLightGhost.tsx` paints three grey dots
  (centres x 20/40/60, top 12) only when `isMac && !active && !fullScreen`;
  the real buttons cover them exactly when AppKit redraws.
- New IPC: minimize/maximize/close plus `window:isMaximized` /
  `window:maximized-changed`, `window:isFullScreen` / `window:fullscreen-changed`,
  `window:isFocused` / `window:active-changed`, consumed only through
  `src/renderer/src/lib/window-chrome.ts` (`isMac`, `isWindows`, `isLinux`,
  `MAC_TRAFFIC_LIGHT_INSET_CLASS = "pl-[76px]"`,
  `useFullScreen()`, `useWindowActive()`; values are cached at module level so
  late-mounting components paint correctly on first render).
- Windows/Linux place `WindowControls` at the window level, outside the stage.
  The stage gets `data-controls-cutout="true"` and mirrors the macOS notch on
  the right (104 × 30 px); the header reserves 112 px so app actions never sit
  underneath the minimize, maximize, and close hit targets.
- The notch: when `isMac && !sidebarOpen && !fullScreen`, `.stage-frame` gets
  `data-notch="true"` and `--nt` transitions 0 → 1 in the same 300 ms as the
  sidebar slide. Geometry: card radius 22, tab 70 × 26 px, convex corner 12,
  fillet 11. The ring (`.stage-ring`, even-odd 1 px hollow), the card
  (`.stage-card`, inset 1 px, radius 21) and the header inset (76 px) and
  ghost dots are all tuned to each other — change them together.
- The shadow is `.stage-shadow`, a transparent rounded-rect sibling with a
  plain `box-shadow`. It is *not* `filter: drop-shadow` on purpose: a filter
  would shadow the translucent card's content (halos under every opaque
  child) and make the frame a containing block for `position: fixed`
  descendants. That is also why `SettingsSheet` is rendered outside the frame.

### 3.4 App shell (`App.tsx`, `Sidebar.tsx`, `WelcomeView.tsx`, `SettingsSheet.tsx`)

- Root is `.app-canvas` with a 10 px gutter (`p-2.5`); the gutter above the
  card is a drag region. Sidebar is a 260 px column that stays mounted and
  collapses with `marginLeft: -260` + `translateX(-10px)` (the extra 10 px
  takes its scrollbar fully off-canvas), `inert` + `aria-hidden` when closed.
  The old pointer-drag resize handle and the sidebar wordmark are gone.
- Header (44 px, absolute, drag region): open-sidebar button when collapsed,
  title, folder chip, a `···` "Thread actions" button (**placeholder, no
  onClick yet**), with space reserved for the window-level controls cutout.
- Content fades out under the title strip via `.title-fade`, a `mask-image`
  on the ChatView and ProxyView scrollers (transparent to 44 px, opaque at
  84 px). A painted scrim was tried first and produced a visible edge in dark
  because no flat colour can match a translucent card over a gradient. The
  scrollers pad `pt-[88px]` so resting content never sits inside the band;
  floating controls (jump button, toasts) must be siblings of a masked
  scroller, not children.
- Thread status: `App.tsx` derives `idle | unread | working` per thread from
  acp-client `ThreadSummary.revision/lifecycle` and
  `ClientSessionState.threadRevision/running`, tracked in a session-scoped
  `seenRevisions` map (threads that advanced while the app was closed are not
  flagged on launch). Sidebar renders `.thread-dot` with `-unread` (brand) or
  `-working` (breathing) modifiers.
- Welcome: coloured falcon symbol (`AxiomMark`, an `<img>`), PP Cirka greeting
  with a CSS line-unmask gated on `document.fonts.load('400 1em "PP Cirka"')`,
  composer, trust strip. Ambient blooms are three static `.bg-blob`s
  (irregular eight-value radii, `blur(90px)`) on a fixed 2000 × 1400
  composition pinned to the view centre, so resizing reveals rather than
  moves them. Do not animate them.
- Settings is a full-window page (`fixed inset-0`, Back pill), not a modal.
  A rounded two-column surface contains a vertical category tablist on the
  left and independently scrolling content on the right. Category metadata
  lives in `SettingsPanel.tsx`; add a category and its panel in `SettingsSheet`.
  Arrow keys / Home / End select categories. Inactive panels stay mounted but
  hidden to preserve local drafts; the chat behind Settings is inert. Window
  controls remain accessible. Account and Appearance are settings categories;
  payment controls live separately in `BalanceScreen.tsx`. Both pages use the
  shared `AccountScreen` shell with a short fade/slide on entry and exit,
  disabled for reduced motion. Closing finishes before restoring chat focus.
- The sidebar account row opens an upward-facing `AccountMenu`, with Settings
  and Balance (including the current available inference credit). There is no
  separate settings gear. The menu refreshes the balance once when opened;
  recurring deposit polling runs only while the Balance screen is mounted.
- Live account state replaces the placeholder identity in the sidebar footer.
  Signed-out, invalid, and unavailable states use the same semantic glass,
  status, and action tokens as the rest of the shell. Sign in opens the website
  directly, where the user chooses their method. `SignInStatus` is an inline
  card below the stage header with the matching code and reopen/cancel/retry
  actions; there is no desktop sign-in modal.

### 3.5 Chat surfaces

- `ChatComposer`: `.composer-shell` glass box (ring/halo/focus stack lives in
  CSS), Lock glyph, chip-style model/thinking controls, `⌘↵` / `Ctrl↵` hint
  (decorative; the handler is bare Enter submits, Shift+Enter newline), 34 px
  ink Send/Stop with `text-on-accent`. The Attach button was removed.
- `MessageBubble`: user messages are glass tiles (`--surface-bubble-user`,
  19 px radius, tucked bottom-right corner, max 80 % width). Assistant replies
  have **no bubble and no avatar**; the `ml-11` gutter is gone everywhere so
  tool rows, activity rows and interaction cards share one left edge. The
  reasoning ("Thought") disclosure sits above the reply; its open panel is a
  `--wash-row` tile, no left rule.
- `ToolCallCard`: same inline-disclosure pattern as Thought (chevron + title +
  status icon; the status word was dropped), details in a `--wash-row` tile.
- `ModelPicker`: 24 rem `.glass-panel-solid` popover, single-line rows,
  selected row marked only by the orange check badge. Monochrome provider
  marks use `.brand-mono` (inverted in dark).
- `TeeVerificationBadge`: one solid `Shield` silhouette filled per state
  (success / danger / warning / muted) with a small check or X in
  `text-on-accent`; the re-verify arrow appears on hover of the badge only.
- `ProxyView`: `shadow-glass-card` wells, ink status disc, `--color-success`
  running dot. Still carries its original mock data (`ERRORS`, `LOCAL_TOKEN`,
  port 8484); the Attestation "inspecting" branch keeps `pt-20` and no
  `title-fade` (inconsistent with the main page, harmless).
- `ChatMarkdown` is untouched, but `.markdown-prose` changed: ink links with
  token underlines, `--wash-code` inline code, `pre` uses `contain: inline-size`
  so wide code cannot widen a bubble, GFM tables.

### 3.6 Brand and packaging

- Design masters live **outside the repo** (`Axiom new UI/brand-desktop/`,
  Dmitry's machine). `pnpm brand:assets` (`scripts/brand-assets.py`, python3 +
  Pillow, macOS `iconutil`/`tiffutil`) regenerates `build/` and
  `resources/icon*.{png,svg}`; see `build/README.md` for the file → consumer
  map. `scripts/rasterize-svg.mjs` rasterises SVGs through the app's own
  Chromium (tray template images).
- `electron-builder.yml`: DMG (660 × 400 window, slots at (200,204) and
  (460,204) around the arrow in the art), NSIS with sidebar/header bitmaps,
  AppImage + deb, sidecar via `extraResources` from `../../target/release/`.
- macOS: minimal application menu (About / Hide / Quit, Edit, Window) and
  `app.setAboutPanelOptions`. Windows/Linux keep no menu. The Edit menu is
  also what makes ⌘C/⌘V reach the renderer on macOS.
- Dev niceties: Dock icon set from `resources/icon-mac.png`; the local
  `Electron.app` bundle is renamed to the product name by
  `scripts/brand-dev-bundle.mjs` (run from `ensure-electron.mjs`; replaces the
  plist file rather than editing it so pnpm's store copy stays pristine).

### 3.7 Removed

Geist fonts, `animejs`, `ogl`, sakura petals, the ember WebGL corner, the
`InputFrost` tracker, the sidebar wordmark and resize handle, the modal
settings dialog, the dark-only `white/[x]` alphas, the Windows title-bar
overlay inset, and the `AxiomLogo` wordmark component.

## 4. Conventions to keep

1. **No literal colours in components.** `@theme` tokens have Tailwind
   utilities (`text-on-accent`, `bg-brand`, `bg-warning-soft`, `text-danger`);
   `:root` semantic variables are consumed as `bg-[var(--surface-tile)]`. A new
   semantic variable goes in **both** the light and the dark block.
2. Anything drawn on `bg-[var(--color-cherry)]` uses `text-on-accent`, never
   `text-white`. `text-white` is only correct on `bg-brand` (orange does not flip).
3. Status is semantic (`--color-success|danger*|warning*`, `--color-text-tertiary`
   for idle), never the accent.
4. Elevation comes from `shadow-glass-tile` / `-card` / `-pop`; glass fills
   from `.glass-panel` (leaf popovers only — it has a backdrop-filter) or
   `.glass-panel-solid` (larger panels). Every new shadow or glass rule ships a
   `:root[data-theme="dark"]` twin.
5. Unlayered classes in `index.css` (`composer-shell`, `ax-pill`, `ax-chip`,
   `shadow-glass-*`, `glass-panel*`, `title-fade`) beat Tailwind utilities on the
   same element. Override in CSS or split the element.
6. Theme state only through `lib/theme.ts`; platform/window state only through
   `lib/window-chrome.ts`. Don't read `window.axiomDesktop.platform` or the IPC
   channels in components.
7. Window-state IPC pattern: `ipcMain.handle("window:isX")` for the initial
   pull, `webContents.send("window:x-changed")` for pushes, preload exposes
   `isX()` + `onXChange(cb) → unsubscribe`.
8. Nothing that hosts `position: fixed` descendants may carry a retained
   `filter`, `backdrop-filter` or `transform` (`.stage-frame`, `.stage-card`,
   `.app-canvas`). `.animate-panel-in` uses `backwards` fill for the same reason.
9. Scrollers under the title strip: add `title-fade`, start content at
   `pt-[88px]`, keep floating controls outside the scroller.
10. Reuse the inline-disclosure recipe (Thought / tool call) for any new
    disclosure in the assistant flow; keep the no-gutter alignment.
11. Every new keyframe goes into the `prefers-reduced-motion` block at the end
    of `index.css`.
12. Dev-only behaviour is gated on `!app.isPackaged`; dev-only args reach the
    sandboxed preload via `additionalArguments` with an `--axiom-` prefix.
13. Identity strings stay in sync: `stream.axiom.desktop` (`APP_ID`, `appId`,
    `StartupWMClass`, desktop entry) and the product name (`APP_NAME` in
    `src/main/index.ts`, `productName` in `package.json`).

## 5. Gotchas

- `clip-path: shape()` and `@property` need Chromium ≥ 135 (Electron 37 =
  138). Downgrading Electron silently removes the notch: the card falls back to
  its `rounded-[21px]` box and the ring paints as a faint rectangle.
- `data-theme` is always `light` or `dark`; `system` is resolved in JS. An
  explicit preference sets `nativeTheme.themeSource`, which also forces
  Chromium's `prefers-color-scheme` — don't read that media query elsewhere
  expecting the OS value.
- `--capture` sends `window:fullscreen-changed` synthetically (true for most
  shots, false for `collapsed`); the renderer must not assume it mirrors the OS
  during capture. Capture output file names changed from `conversation.png`
  to `light-conversation.png` / `dark-conversation.png` etc.
- `useFullScreen` / `useWindowActive` never unsubscribe on purpose (module
  cache); `window:isFocused` defaults to `true` so ghosts never flash.
- `SIDEBAR_WIDTH` (260) is duplicated as the `w-[260px]` class; change both.
- `inert={!sidebarOpen}` relies on React 19's boolean `inert`.
- `brandDevBundle()` edits `node_modules/electron/dist/Electron.app` on every
  `pnpm desktop`; a fresh `pnpm install` resets it until the next run. macOS only.
- The `arm-pulse` keyframes and `.ax-dot*` / `.ax-meta` classes have no
  consumer yet (parity with the webapp).

## 6. Open items for the developer

- **Copyright line** is a placeholder (`© 2026 Astrea Foundation`, marked
  `TODO(brand)` in `src/main/index.ts` and duplicated in `electron-builder.yml`).
- **Product name**: the code says `Axiom`; the wordmark reads `AxiomAI`.
  Decide, then change `productName` + `APP_NAME` (+ DMG/NSIS titles in the yml).
- **macOS 26 Liquid Glass icon**: `build/tahoe-icon-layers/` is ready for
  Icon Composer (Xcode 26); `mac.icon` still points at `icon.icns`, which
  macOS 26 masks itself — fine as a fallback.
- The header `···` "Thread actions" button needs a menu.
- `ProxyView` still shows mock data; the "inspecting" branch could take
  `title-fade` + `pt-[88px]` like the main page.
- Linux: `build/icons` replaces the old `resources/icon.png` mark; re-run
  `pnpm desktop:install` if the dev launcher looks stale.

## 7. Not in this branch (on purpose)

- Interactive native account, provider, and target-machine validation is not
  part of the headless design pass. The current client supports the registered
  NEAR and Phala provider-E2EE protocols and must never fall back to plaintext.
- Keychain: an ad-hoc-signed rebuild of `axiomcli` can invalidate the native
  refresh token's ACL, so a development build may require native sign-in again
  from Settings. This is expected for ad-hoc builds.
- The mock sidecar used for design review (see §2).

## 8. How this was verified

Current headless verification is
`pnpm --filter axiom-desktop typecheck` plus the 24/24 integrated Desktop tests.
The original redesign captured 24 states across both themes and received a
five-lens adversarial code review over the whole diff (CSS/Chromium support,
React/TS, Electron main/preload, token completeness/contrast, behavioural
regressions). Visible capture and target-machine testing remain separate and
must wait for an authorized manual test window.
