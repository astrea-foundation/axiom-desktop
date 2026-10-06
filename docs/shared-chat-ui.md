# Shared chat presentation

`packages/chat-ui` owns the renderer's portable Markdown/code/math rendering,
model picker, model/provider icons, greeting, title overflow, model presentation
types and visual stylesheet. Desktop imports these through `@axiom/chat-ui`;
the original renderer paths re-export them to preserve existing callers/tests.
Native account, ACP, persistence, security and tool behavior stay in Desktop.

The package ships TypeScript/TSX source for React/Vite consumers and expects
React and `@axiom/brand` from the consuming workspace. Import its `styles.css`
with Tailwind 4, and include the package's source in Tailwind's source scan.
No Electron globals or native SDK types enter the shared package.

Run `pnpm --filter @axiom/chat-ui typecheck` and Desktop's normal typecheck/tests
after editing it. `pnpm --filter @axiom/chat-ui pack --pack-destination <dir>`
creates a versioned artifact for another independently buildable checkout.
Hosted applications remain in `axiom-platform`; they consume a pinned package
artifact, never source paths into a neighboring Desktop checkout. Record source
revision and artifact SHA256 with that artifact. This is UI packaging, not a
Desktop release or installer-version change.
