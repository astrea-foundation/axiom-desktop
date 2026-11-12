# Shared chat presentation and behavior

`packages/chat-ui` owns the actual Desktop composer, welcome/chat views,
sidebar and context/account menus, transcript/reasoning/tool cards, queue,
attachment cards, privacy proof, Web consent, rename/delete dialogs, usage and
settings presentation. Desktop consumes these components through preserved
renderer re-exports. Hosted applications consume the same committed artifact.

`packages/chat-core` owns structural runtime views, host capabilities/ports,
attachment validation and original-file reading, the existing durable message
queue, thread ordering/loading, reasoning reconciliation, payload interfaces and the canonical chat prompt assets.
AxiomCLI embeds the shared base and Web-off rules with its native capability
profile. Browser hosts consume the same packaged Markdown and add current model,
provider, thinking, fixed conversation start time and web-only tool context.
Prompts are built by the owning client before attested E2EE; the backend does
not construct or inject model messages.
ACP remains a native adapter: no Node transport or Electron implementation is
included in either package. `ChatHostProvider` injects native attachment reads,
usage, interaction resolution and window chrome; the browser supplies its own
ports. Agent/MCP presentation is enabled only when a host supplies that control.
Per-conversation MCP selection lives inside the Agent dialog and remains usable
with Agent mode off; the composer has no separate Tools control. Connection
management stays in Settings.

`VaultScreen` supplies callback-driven first-use password confirmation, recovery
code download/copy and saved-code acknowledgment, password unlock, recovery with
a new password/code, and destructive reset confirmation. Native local storage
has no new password requirement. Encryption and reset authorization belong to
the browser vault adapter and Platform backend, never these forms.

Setup explains that this password protects saved web chats independently of
account sign-in, shows a live checklist for the twelve-character minimum and matching passwords and identifies
the password/recovery-code steps. Show-password controls, password-manager field
sections, matching feedback and Caps Lock hints help users avoid typos. Failed
attempts preserve transient input for correction; successful steps, cancellation
and navigation clear secrets. Inputs and navigation cannot change during a pending
attempt. Code copying/downloads show feedback, with selectable code as a clipboard
fallback. Recovery explains replacement of the password/code while keeping chats.
Reset shows a five-second reading countdown entirely in the client before enabling
its confirmed delete action. It does not request a server challenge or require
another sign-in. Hosts supply `onSignIn` only for an actually expired account session.
Password changes and recovery use the existing active account session.
The card scrolls from the top when it exceeds the available viewport height.

The package ships TypeScript/TSX for React/Vite. Import its `styles.css` with
Tailwind 4 and include both packages in the consumer's source scan. Theme,
keyboard/focus, Markdown/code/math, provider logos and privacy states retain
Desktop behavior. The provider proof dialog distinguishes gateway evidence from
upstream verification when a gateway adapter supplies that scope.

Sidebar hosts may supply `aboveAccount` content immediately above the account
menu. Its rendering, media and dismissal lifecycle belong to the host; the
shared sidebar provides only the optional layout slot.

Desktop's Electron main build must bundle both `@axiom/axiom-acp-client` and
its transitive `@axiom/chat-core` imports into JavaScript. These packages export
TypeScript source for bundlers; packaged Node cannot load their `.ts` exports
from `node_modules`. The Desktop unit suite builds the production main entry
and checks that shared attachment validation is included without runtime
workspace-package or TypeScript imports.

The settings screen and category navigation are shared too. Hosts supply the
available categories and account/storage actions. Web hides Agent, MCP, Proxy,
native updates and local API-key controls. Unsupported uploads are disabled with
provider/model-specific hover text. Composer relocation animates on immediate
thread creation and respects reduced motion.
The composer disables model selection during preparation and streaming, dismisses
an open model picker when either starts, and restores selection when the turn
ends. Hosts must keep `isStreaming` true through tool calls and cancellation
cleanup and reject model changes against an active run in their settings handler.
Hosts may supply `onEnsureSecurity` for automatic, cache-aware report renewal;
`onVerifySecurity` remains the explicit Refresh action. Without that optional
callback, automatic renewal retains the existing verification callback. An
already valid report stays visible during automatic renewal until its true
hard expiry. Running image/tool turns finish before either refresh can start.

Queue hosts may provide `persistQueue` to require an encrypted durable commit
before dispatch. Native hosts retain their existing synchronous admission timing.
Only an active preparation may bind a thread or become ready for dispatch.
Restoring an outbox after an account/connection change converts preparations to
queued drafts; late callbacks cannot advance them, even if the same account and
runtime return. Reviewing such a draft transfers it to the composer and still
requires an explicit Send.

Run the workspace typecheck, Desktop unit tests and affected browser suites.
Pack committed packages with `pnpm --filter @axiom/chat-core pack` and
`pnpm --filter @axiom/chat-ui pack`, recording source revision, SHA256 and lock
integrity in the consuming repository. Both repositories build independently;
there are no neighboring-checkout source imports or submodules. Package artifact
versions do not bump the Desktop product or publish installers.

The vault form also accepts an explicit `change-password` mode and cancellation
callback for a host that has already unlocked and paused its runtime. Key rotation
remains host-owned and must finish atomic encrypted storage commit before returning
to chat. Native local storage does not adopt the web vault.
